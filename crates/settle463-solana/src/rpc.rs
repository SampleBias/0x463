use super::*;
use crate::transaction::{config_message,decode_wire};
use base64::{engine::general_purpose::STANDARD,Engine};
use ed25519_dalek::{Signer,SigningKey};
use serde_json::{json,Value};
use std::{io::Read, sync::Arc, time::Duration};

pub(crate) trait Transport:Send+Sync { fn call(&self,method:&str,params:Value)->Result<Value>; }
struct Http { endpoint:String, agent:ureq::Agent }
impl Transport for Http {
    fn call(&self,method:&str,params:Value)->Result<Value> {
        let err=|e:&dyn std::fmt::Display| Error::Unavailable(format!("{method}: {e}"));
        let response=self.agent.post(&self.endpoint)
            .send_json(json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
            .map_err(|e|err(&e))?;
        let mut bytes=Vec::new();
        response.into_reader().take(4*1024*1024+1).read_to_end(&mut bytes).map_err(|e|err(&e))?;
        if bytes.len()>4*1024*1024 {return Err(err(&"response size limit"));}
        let v:Value=serde_json::from_slice(&bytes).map_err(|e|err(&e))?;
        if v["jsonrpc"]!="2.0" || v["id"]!=1 || v.get("error").is_some() {return Err(err(&v["error"]));}
        v.get("result").cloned().ok_or_else(||err(&"missing result"))
    }
}

pub(crate) struct Account {pub owner:String,pub data:Vec<u8>,pub executable:bool}
#[derive(Clone)]
pub struct Rpc { pub(crate) deployment:Deployment, transport:Arc<dyn Transport> }
impl Rpc {
    pub fn new(deployment:&Deployment)->Result<Self>{
        deployment.check()?;
        let url=url::Url::parse(&deployment.rpc_url).map_err(invalid)?;
        let loopback=match url.host(){Some(url::Host::Ipv4(ip))=>ip.is_loopback(),Some(url::Host::Ipv6(ip))=>ip.is_loopback(),_=>false};
        ensure(loopback && matches!(url.scheme(),"http"|"https") && url.username().is_empty() && url.password().is_none() && url.fragment().is_none(),"private loopback RPC required")?;
        let agent=ureq::AgentBuilder::new().timeout(Duration::from_secs(12)).redirects(0).build();
        Ok(Self{deployment:deployment.clone(),transport:Arc::new(Http{endpoint:url.to_string(),agent})})
    }
    pub(crate) fn call(&self,method:&str,params:Value)->Result<Value>{self.transport.call(method,params)}
    pub(crate) fn account(&self,address:&str,minimum:u64)->Result<Option<Account>>{
        let result=self.call("getAccountInfo",json!([address,{"encoding":"base64","commitment":"finalized","minContextSlot":minimum}]))?;
        ensure(result["context"]["slot"].as_u64().is_some_and(|slot|slot>=minimum),"stale account context")?;
        let v=result.get("value").ok_or_else(||invalid("missing account response"))?;
        if v.is_null(){return Ok(None);}
        ensure(v["data"][1]=="base64","account encoding")?;
        Ok(Some(Account{data:STANDARD.decode(v["data"][0].as_str().ok_or_else(||invalid("account data"))?).map_err(invalid)?,
            owner:v["owner"].as_str().ok_or_else(||invalid("account owner"))?.to_owned(),executable:v["executable"].as_bool().ok_or_else(||invalid("account executable flag"))?}))
    }
    fn check_runtime(&self)->Result<()>{
        let d=&self.deployment;
        ensure(self.call("getGenesisHash",json!([]))?==d.genesis,"RPC genesis disagrees with trusted deployment")?;
        let program=self.account(&d.program_id,0)?.ok_or_else(||invalid("missing program"))?;
        let program_data=Pubkey::find_program_address(&[key(&d.program_id)?.as_ref()],&key(LOADER)?).0;
        ensure(program.executable && program.owner==LOADER && program.data.get(..4)==Some(&[2,0,0,0]) && program.data.get(4..36)==Some(program_data.as_ref()),"upgradeable program account")?;
        let mint=self.account(&d.mint,0)?.ok_or_else(||invalid("missing mint"))?;
        ensure(mint.owner==TOKEN_PROGRAM && !mint.executable && mint.data.len()==82 && mint.data[44]==6 && mint.data[45]==1,"classic six-decimal initialized mint")?;
        Ok(())
    }
    pub fn validate_deployment(&self)->Result<()>{
        let d=&self.deployment;self.check_runtime()?;
        let config=self.account(&d.config,0)?.ok_or_else(||invalid("missing deployment config"))?;
        let (_,bump)=Pubkey::find_program_address(&[b"settle463",b"config",b"v1"],&key(&d.program_id)?);
        ensure(config.owner==d.program_id && !config.executable && config.data.len()==106 && config.data[..8]==discriminator("account","Config") && config.data[8]==bump && config.data[9..41]==key(&d.mint)?.to_bytes() && config.data[41..73]==key(TOKEN_PROGRAM)?.to_bytes() && config.data[73]==6 && config.data[74..106]==base58::<32>(&d.genesis).map_err(invalid)?,"deployment config, mint or genesis mismatch")
    }
    pub fn clock(&self)->Result<i64>{
        let a=self.account(CLOCK,0)?.ok_or_else(||invalid("clock sysvar missing"))?;
        ensure(a.owner=="Sysvar1111111111111111111111111111111111111" && !a.executable && a.data.len()==40,"clock sysvar")?;
        Ok(i64::from_le_bytes(a.data[32..40].try_into().map_err(invalid)?))
    }
    pub fn balance(&self,ata:&str)->Result<u64>{
        self.token_balance(ata,None,0).map(|b|b.unwrap_or_default())
    }
    pub(crate) fn token_balance(&self,address:&str,owner:Option<&Pubkey>,slot:u64)->Result<Option<u64>> {
        key(address)?;
        let Some(a)=self.account(address,slot)? else {return Ok(None)};
        ensure(a.owner==TOKEN_PROGRAM && !a.executable && a.data.len()==165 && a.data[..32]==key(&self.deployment.mint)?.to_bytes() && matches!(a.data[108],1|2) && owner.is_none_or(|k|k.as_ref()==&a.data[32..64]),"token account program, mint or authority")?;
        Ok(Some(u64::from_le_bytes(a.data[64..72].try_into().map_err(invalid)?)))
    }
    fn latest(&self)->Result<(String,u64,u64)>{
        let v=self.call("getLatestBlockhash",json!([{"commitment":"finalized"}]))?;
        let hash=v["value"]["blockhash"].as_str().ok_or_else(||invalid("blockhash"))?;
        base58::<32>(hash).map_err(invalid)?;
        Ok((hash.to_owned(),v["value"]["lastValidBlockHeight"].as_u64().ok_or_else(||invalid("last valid height"))?,v["context"]["slot"].as_u64().ok_or_else(||invalid("blockhash context slot"))?))
    }
    fn eligible(&self,e:&Expected)->Result<()> {
        ensure(e.deployment==self.deployment,"different signing deployment")?;
        self.validate_deployment()?;
        let now=self.clock()?;
        let [fund,deliver,resolve]=e.times()?;
        let view=self.inspect(e)?;
        use settle463_core::settlement::OrderState;
        let ok=match e.action {
            ChainAction::Fund=>view.is_none() && now<fund && fund<deliver && deliver<resolve && e.principal()?>0,
            ChainAction::CommitDelivery=>view.as_ref().is_some_and(|v|v.state==OrderState::Funded)&&now<deliver,
            ChainAction::Release=>view.as_ref().is_some_and(|v|v.state==OrderState::Delivered && v.delivery_id==e.delivery.map(|d|hex_string(&d))) && now<resolve,
            ChainAction::Refund=>view.as_ref().is_some_and(|v|v.delivery_id==e.delivery.map(|d|hex_string(&d)) && match v.state {OrderState::Funded=>now>=deliver,OrderState::Delivered=>now>=resolve,_=>false}),
        };
        ensure(ok,"escrow state or ledger deadline disallows action")
    }
    pub fn prepare(&self,e:&Expected)->Result<Prepared>{
        self.eligible(e)?;let (hash,height,slot)=self.latest()?;
        e.build(&hash,height,slot)
    }
    pub fn sign(&self,e:&Expected,p:&Prepared,seed:&[u8;32])->Result<String>{
        self.eligible(e)?;
        let raw=STANDARD.decode(&p.message).map_err(invalid)?;
        let m=crate::transaction::decode_message(&raw)?;
        let is_valid=self.call("isBlockhashValid",json!([m.recent_blockhash.to_string(),{"commitment":"finalized","minContextSlot":p.context_slot}]))?;
        ensure(is_valid["value"]==true,"blockhash expired before signature")?;
        e.sign(p,seed)
    }
    /// Broadcast identical, already-signed bytes. The coordinator commits its intent and outbox first.
    pub fn broadcast(&self,tx:&SignedTx)->Result<()> {
        let (_,wire)=decode_wire(&tx.wire)?;
        ensure(base58::<64>(&tx.signature).map_err(invalid)?.as_slice()==&wire[1..65],"wrong transaction locator")?;
        self.send(&tx.wire,&tx.signature)
    }
    fn send(&self,wire:&str,signature:&str)->Result<()> {
        let s=self.call("sendTransaction",json!([wire,{"encoding":"base64","skipPreflight":true,"maxRetries":0}]))?;
        ensure(s==signature,"RPC returned different signature")
    }
    pub fn initialize_config(&self,seed:&[u8;32])->Result<String>{
        self.check_runtime()?;
        let signer=SigningKey::from_bytes(seed);
        let authority=Pubkey::new_from_array(signer.verifying_key().to_bytes());
        let m=config_message(&self.deployment,authority,&self.latest()?.0)?;
        let bytes=m.serialize();
        let sig=signer.sign(&bytes).to_bytes();
        let mut wire=vec![1];wire.extend(sig);wire.extend(bytes);
        let signature=bs58::encode(sig).into_string();
        self.send(&STANDARD.encode(wire),&signature)?;
        Ok(signature)
    }
    pub fn find_refund(&self,e:&Expected)->Result<Option<(String,Resolution)>> {
        ensure(e.action==ChainAction::Refund && e.payer.is_none(),"external refund search context")?;
        self.validate_deployment()?;
        let mut before=None::<String>;
        loop {
            let mut opts=json!({"commitment":"finalized","limit":100});
            if let Some(cursor)=&before {opts["before"]=cursor.as_str().into();}
            let page=self.call("getSignaturesForAddress",json!([e.terms.escrow_account,opts]))?;
            let rows=page.as_array().ok_or_else(||invalid("signature page"))?;
            for row in rows {
                let sig=row["signature"].as_str().ok_or_else(||invalid("missing signature"))?;
                if !row["err"].is_null() {continue;}
                match self.reconcile(e,sig,None) {
                    Ok(r) if r.outcome==ChainOutcome::FinalizedSuccess =>return Ok(Some((sig.to_owned(),r))),
                    Ok(_)|Err(Error::Invalid(_))=>(),
                    Err(other)=>return Err(other),
                }
            }
            if rows.len()<100 {return Ok(None);}
            let next=rows.last().and_then(|r|r["signature"].as_str()).ok_or_else(||invalid("signature cursor"))?;
            ensure(before.as_deref()!=Some(next),"repeated RPC cursor")?;
            before=Some(next.to_owned());
        }
    }
}
