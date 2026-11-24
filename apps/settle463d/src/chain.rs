use super::{actions, db, *};
use base64::{engine::general_purpose::STANDARD, Engine};
use settle463_core::{
    ident::{decimal, hex},
    receipt::SolanaLedger,
    settlement::{Agreement, ChainOutcome, OrderState, OrderView, Review},
    wire::Settlement,
    SignedMessage,
};
use settle463_solana::{ChainAction, Deployment, Expected, Resolution, Rpc, SignedTx};
use sqlx::Row;

pub(super) struct Chain {
    deployment: Deployment,
    rpc: Rpc,
}

fn chain_call<T>(f:impl FnOnce()->std::result::Result<T,settle463_solana::Error>) -> Result<T> {
    tokio::task::block_in_place(f).map_err(|e| {
        if e.is_unavailable() { ApiError(StatusCode::SERVICE_UNAVAILABLE,"LEDGER_UNAVAILABLE") } else { ApiError::link() }
    })
}

struct Evidence {
    agreement: Agreement,
    delivery: Option<SignedMessage>,
    verdict: Option<SignedMessage>,
}
impl Evidence {
    async fn load(s:&State, db:&mut Conn,order:&str,delivery:Option<&str>,verdict:Option<&str>) -> Result<Self>{
        let env=&s.config.environment;
        Ok(Self {
            agreement:db::agreement(db,env,order).await?,
            delivery:match delivery {Some(i)=>Some(db::load(db,i,env).await?),None=>None},
            verdict:match verdict {Some(i)=>Some(db::load(db,i,env).await?),None=>None},
        })
    }
    fn expected(&self,chain:&Chain,action:ChainAction,external:bool)->Result<Expected> {
        let (_,q)=self.agreement.terms(&chain.deployment.environment())?;
        let payer=match action {
            ChainAction::Fund|ChainAction::Refund=>&q.buyer,
            ChainAction::CommitDelivery=>&q.seller,
            ChainAction::Release=>&q.verifier,
        };
        let payer=if external{String::new()}else{bs58::encode(hex::<32>(payer)?).into_string()};
        Expected::new(&chain.deployment,&self.agreement,action,self.delivery.as_ref(),self.verdict.as_ref(),&payer).map_err(|_|ApiError::link())
    }
}
fn action(name:&str)->Result<ChainAction>{
    match name {
        "fund"=>Ok(ChainAction::Fund),"commit_delivery"=>Ok(ChainAction::CommitDelivery),
        "release"=>Ok(ChainAction::Release),"refund"=>Ok(ChainAction::Refund),_=>Err(ApiError::bad("BAD_ENCODING")),
    }
}
async fn inputs(s:&State,db:&mut Conn,order:&str,v:&Value,submit:bool)->Result<(ChainAction,Evidence)> {
    let name=field(v,"action")?;
    let kind=action(name)?;
    let expected_keys=1+usize::from(submit)+usize::from(matches!(kind,ChainAction::CommitDelivery|ChainAction::Release));
    if v.as_object().is_none_or(|o|o.len()!=expected_keys) {return Err(ApiError::bad("UNKNOWN_FIELD"));}
    let row=sqlx::query("SELECT delivery_id,definitive_id FROM orders WHERE order_id=?").bind(order).fetch_one(&mut *db).await?;
    let prior:Option<String>=row.get("delivery_id");
    let (d,v)=match kind {
        ChainAction::Fund => (None,None),
        ChainAction::CommitDelivery=>(Some(field(v,"delivery_id")?),None),
        ChainAction::Release=>{
            let sticky:Option<String>=row.get("definitive_id");
            let id=field(v,"verdict_id")?;
            if sticky.as_deref()!=Some(id) {return Err(ApiError::link());}
            (Some(prior.as_deref().ok_or_else(ApiError::link)?),Some(id))
        }
        ChainAction::Refund=>(prior.as_deref(),None),
    };
    Ok((kind,Evidence::load(s,db,order,d,v).await?))
}

impl Chain {
    pub(super) fn new(deployment:&Deployment,env:&str)->std::result::Result<Self,BoxError> {
        if deployment.environment()!=env {return Err("deployment does not match pinned environment".into());}
        let rpc=Rpc::new(deployment)?;
        Ok(Self{deployment:deployment.clone(),rpc})
    }
    pub(super) fn clock(&self)->Result<i64>{chain_call(|| self.rpc.clock())}
    pub(super) fn balance(&self,ata:&str)->Result<u64>{chain_call(|| self.rpc.balance(ata))}
    pub(super) async fn prepare(&self,s:&State,db:&mut Conn,order:&str,v:&Value)->Result<Vec<u8>>{
        let (action,inputs)=inputs(s,db,order,v,false).await?;
        let expected=inputs.expected(self,action,false)?;
        let draft=chain_call(||self.rpc.prepare(&expected))?;
        json_bytes(json!({"message":draft.message,"last_valid_block_height":draft.last_valid_block_height.to_string()}))
    }
    pub(super) async fn enqueue(&self,s:&State,db:&mut Conn,order:&str,cmd:&Value)->Result<String> {
        let tx=&cmd["transaction"];
        if tx.as_object().is_none_or(|o|o.len()!=3){return Err(ApiError::bad("BAD_ENCODING"));}
        let signed=SignedTx {wire:field(tx,"wire")?.into(),signature:field(tx,"signature")?.into(), last_valid_block_height:decimal(field(tx,"last_valid_block_height")?)?};
        let name=field(cmd,"action")?;
        let id=hex_string(&message_id(signed.signature.as_bytes()));
        let raw=STANDARD.decode(&signed.wire).map_err(|_|ApiError::bad("BAD_ENCODING"))?;
        if let Some(old)=sqlx::query("SELECT order_id,action,signed_bytes FROM intents WHERE intent_id=?").bind(&id).fetch_optional(&mut *db).await?{
            let same=old.get::<String,_>("order_id")==order && old.get::<String,_>("action")==name && old.get::<Vec<u8>,_>("signed_bytes")==raw;
            return if same{Ok(id)}else{Err(ApiError::conflict("STATE_CONFLICT"))};
        }
        let (kind,inputs)=inputs(s,db,order,cmd,true).await?;
        let expected=inputs.expected(self,kind,false)?;
        expected.validate_signed(&signed).map_err(|_|ApiError::auth())?;
        let delivery=inputs.delivery.as_ref().map(|m|hex_string(m.id()));
        let verdict=inputs.verdict.as_ref().map(|m|hex_string(m.id()));
        let rows=sqlx::query("INSERT OR IGNORE INTO intents(intent_id,order_id,action,state,signed_bytes,local_signature,last_valid_block_height,delivery_message,verdict_message) VALUES(?,?,?,'prepared',?,?,?,?,?)")
            .bind(&id).bind(order).bind(name).bind(raw).bind(&signed.signature).bind(signed.last_valid_block_height.to_string()).bind(delivery).bind(verdict).execute(&mut *db).await?.rows_affected();
        if rows!=1{return Err(ApiError::conflict("STATE_CONFLICT"));}
        sqlx::query("INSERT INTO outbox(intent_id,available_at) VALUES(?,?)").bind(&id).bind(http::now()?.to_string()).execute(&mut *db).await?;
        db::audit(s,db,order,"intent_recorded",&json!({"action":name,"intent_id":id,"signature":signed.signature}),vec![]).await?;
        Ok(id)
    }
    pub(super) async fn process(&self,s:&State,id:&str)->Result<()>{
        let mut db=s.pool.begin_with("BEGIN IMMEDIATE").await?;
        let row=sqlx::query("SELECT * FROM intents WHERE intent_id=?").bind(id).fetch_one(&mut *db).await?;
        let state:String=row.get("state");
        if matches!(state.as_str(),"finalized_success"|"finalized_failure"|"expired"){return Ok(());}
        let order:String=row.get("order_id");let name:String=row.get("action");
        let kind=action(&name)?;
        let inputs=Evidence::load(s,&mut db,&order,row.get("delivery_message"),row.get("verdict_message")).await?;
        let expected=inputs.expected(self,kind,false)?;
        let signed=stored_tx(&row)?;
        let resolution=chain_call(|| self.rpc.reconcile(&expected,&signed.signature,Some(&signed)))?;
        if resolution.outcome==ChainOutcome::FinalizedSuccess {
            self.finalize(s,&mut db,&row,&inputs,kind,&signed.signature,&resolution).await?;
        } else if matches!(resolution.outcome,ChainOutcome::Expired|ChainOutcome::FinalizedFailure) {
            let terminal=if resolution.outcome==ChainOutcome::Expired{"expired"}else{"finalized_failure"};
            sqlx::query("UPDATE intents SET state=?,finalized_slot=? WHERE intent_id=?").bind(terminal).bind(resolution.slot.map(|n|n.to_string())).bind(id).execute(&mut *db).await?;
        } else {
            let attempts:i64=row.get("attempts");
            // The signed bytes are durable before this call. Retransmission uses the same signature.
            let sent=chain_call(||self.rpc.broadcast(&signed)).is_ok();
            #[cfg(debug_assertions)]
            if sent && std::env::var("SETTLE463_TEST_STOP").as_deref()==Ok(format!("after_broadcast:{name}").as_str()){
                std::process::exit(87);
            }
            let status=if sent || resolution.outcome==ChainOutcome::Pending{"submitted"}else{"unknown"};
            sqlx::query("UPDATE intents SET state=?,attempts=attempts+1 WHERE intent_id=?").bind(status).bind(id).execute(&mut *db).await?;
            let delay=1_i64<<attempts.clamp(0,3);
            sqlx::query("UPDATE outbox SET available_at=?, attempts=attempts+1 WHERE intent_id=?").bind((http::now()?+delay).to_string()).bind(id).execute(&mut *db).await?;
            db.commit().await?;
            return Ok(());
        }
        sqlx::query("DELETE FROM outbox WHERE intent_id=?").bind(id).execute(&mut *db).await?;
        let response=actions::status(&mut db,&order,id).await?;
        sqlx::query("UPDATE idempotency SET response=? WHERE intent_id=? AND length(response)=0").bind(response).bind(id).execute(&mut *db).await?;
        db.commit().await?;
        Ok(())
    }
    async fn finalize(&self,s:&State,db:&mut Conn,row:&sqlx::sqlite::SqliteRow,e:&Evidence,kind:ChainAction,signature:&str,res:&Resolution)->Result<()> {
        let slot=res.slot.ok_or_else(ApiError::link)?;
        let clock=res.clock.ok_or_else(ApiError::link)?;
        let review=Review::from_agreement(&e.agreement,&s.config.environment)?;
        let amount=decimal(&review.principal)?;
        let (state,held,paid,refunded)=match kind {
            ChainAction::Fund=>(OrderState::Funded,amount,0,0),
            ChainAction::CommitDelivery=>(OrderState::Delivered,amount,0,0),
            ChainAction::Release=>(OrderState::Paid,0,amount,0),
            ChainAction::Refund=>(OrderState::Refunded,0,0,amount),
        };
        let terminal=matches!(state,OrderState::Paid|OrderState::Refunded);
        let view=OrderView{state,held,paid,refunded,review,
            delivery_id:e.delivery.as_ref().map(|d|hex_string(d.id())),
            verdict_id:e.verdict.as_ref().map(|v|hex_string(v.id())),
            terminal_locator:terminal.then(||signature.to_owned()),terminal_clock:terminal.then_some(clock)};
        let ledger=if terminal {Some(self.ledger(s,db,e,&view,slot).await?)}else{None};
        let order=&view.review.order_id;
        let old=match kind {ChainAction::Fund=>"Accepted",ChainAction::CommitDelivery=>"Funded",ChainAction::Release=>"Delivered",
            ChainAction::Refund if e.delivery.is_some()=>"Delivered",ChainAction::Refund=>"Funded"};
        let version:i64=sqlx::query_scalar("SELECT version FROM orders WHERE order_id=?").bind(order).fetch_one(&mut *db).await?;
        let n=sqlx::query("UPDATE orders SET state=?,held=?,paid=?,refunded=?,delivery_id=?,verdict_id=?,terminal_clock=?,terminal_locator=?,version=version+1
                            WHERE order_id=? AND state=? AND version=?").bind(format!("{state:?}")).bind(held.to_string()).bind(paid.to_string()).bind(refunded.to_string())
            .bind(&view.delivery_id).bind(&view.verdict_id).bind(view.terminal_clock.map(|v|v.to_string())).bind(&view.terminal_locator)
            .bind(order).bind(old).bind(version).execute(&mut *db).await?.rows_affected();
        if n!=1{return Err(ApiError::conflict("STATE_CONFLICT"));}
        let id:&str=row.get("intent_id");
        sqlx::query("UPDATE intents SET state='finalized_success', finalized_slot=?, execution_clock=? WHERE intent_id=?").bind(slot.to_string()).bind(clock.to_string()).bind(id).execute(&mut *db).await?;
        db::audit(s,db,order,"finalized",&json!({"intent_id":id,"signature":signature,"slot":slot.to_string()}),vec![]).await?;
        if let Some(ledger)=ledger { actions::receipt(s,db,&view,Some(&ledger)).await?; }
        Ok(())
    }
    async fn prior(&self,db:&mut Conn,e:&Evidence,order:&str,name:&str)->Result<String>{
        let row=sqlx::query("SELECT * FROM intents WHERE order_id=? AND action=? AND state='finalized_success'").bind(order).bind(name).fetch_one(&mut *db).await?;
        let expected_inputs=Evidence{agreement:e.agreement.clone(),delivery:if name=="commit_delivery"{e.delivery.clone()}else{None},verdict:None};
        let exp=expected_inputs.expected(self,action(name)?,false)?;
        let tx=stored_tx(&row)?;
        let r=chain_call(||self.rpc.reconcile(&exp,&tx.signature,Some(&tx)))?;
        if r.outcome!=ChainOutcome::FinalizedSuccess {return Err(ApiError(StatusCode::SERVICE_UNAVAILABLE,"LEDGER_UNAVAILABLE"));}
        Ok(tx.signature)
    }
    async fn ledger(&self,s:&State,db:&mut Conn,e:&Evidence,view:&OrderView,slot:u64)->Result<SolanaLedger>{
        let (_,q)=e.agreement.terms(&s.config.environment)?;
        let Settlement::Solana{vault_token_account,..}=&q.settlement else { return Err(ApiError::link()); };
        let funding_signature=self.prior(db,e,&q.order_id,"fund").await?;
        let delivery_signature=if view.delivery_id.is_some(){Some(self.prior(db,e,&q.order_id,"commit_delivery").await?)}else{None};
        let paid=view.state==OrderState::Paid;
        Ok(SolanaLedger{ledger_kind:"solana".into(),genesis:self.deployment.genesis.clone(),program_id:self.deployment.program_id.clone(),config:self.deployment.config.clone(),order_pda:view.review.escrow_account.clone(),mint:q.asset.id.clone(),amount:q.amount.clone(),beneficiary:if paid{view.review.seller_destination.clone()}else{view.review.buyer_destination.clone()},vault:vault_token_account.clone(),funding_signature,delivery_signature,terminal_signature:view.terminal_locator.clone().ok_or_else(ApiError::link)?,terminal_instruction:if paid{"release"}else{"refund"}.into(),finalized_slot:slot.to_string(),evidence_digest:String::new()}.seal()?)
    }
    pub(super) async fn discover_refunds(&self,s:&State)->Result<()>{
        let orders:Vec<(String,Option<String>)>=sqlx::query_as("SELECT order_id,delivery_id FROM orders WHERE state IN ('Funded','Delivered') AND NOT EXISTS (SELECT 1 FROM outbox JOIN intents USING(intent_id) WHERE intents.order_id=orders.order_id AND intents.action='refund')").fetch_all(&s.pool).await?;
        for (order,delivery) in orders{
            let mut db=s.pool.begin_with("BEGIN IMMEDIATE").await?;
            let e=Evidence::load(s,&mut db,&order,delivery.as_deref(),None).await?;
            let (_,q)=e.agreement.terms(&s.config.environment)?;
            let due=if delivery.is_some(){&q.deadlines.resolve_by}else{&q.deadlines.deliver_by};
            if self.clock()?<settle463_core::ident::timestamp(due)?{continue;}
            let expected=e.expected(self,ChainAction::Refund,true)?;
            let Some((sig,res))=chain_call(||self.rpc.find_refund(&expected))? else{continue;};
            if res.outcome!=ChainOutcome::FinalizedSuccess{continue;}
            let id=hex_string(&message_id(sig.as_bytes()));
            sqlx::query("INSERT INTO intents(intent_id,order_id,action,state,signed_bytes,local_signature,delivery_message) VALUES(?,?,'refund','observed',x'',?,?) ON CONFLICT(intent_id) DO NOTHING")
                .bind(&id).bind(&order).bind(&sig).bind(&delivery).execute(&mut *db).await?;
            let row=sqlx::query("SELECT * FROM intents WHERE intent_id=?").bind(&id).fetch_one(&mut *db).await?;
            self.finalize(s,&mut db,&row,&e,ChainAction::Refund,&sig,&res).await?;
            db.commit().await?;
        }
        Ok(())
    }
}
fn stored_tx(row:&sqlx::sqlite::SqliteRow)->Result<SignedTx>{
    Ok(SignedTx { signature:row.get("local_signature"),wire:STANDARD.encode(row.get::<Vec<u8>,_>("signed_bytes")),last_valid_block_height:decimal(row.get("last_valid_block_height"))? })
}
