mod demo;
mod session;

use serde_json::{json, Value};
use settle463_core::{
    ident::{environment, hex},
    receipt::{verify_sim_bundle, ReceiptError, ReceiptTrust},
};
use settle463_sdk::{HttpCoordinator, Role};
use std::{collections::BTreeMap, error::Error, fs, path::Path, process::ExitCode};

type AnyResult<T> = Result<T, Box<dyn Error>>;
struct Options {
    args: Vec<String>,
    values: BTreeMap<String, String>,
    json: bool,
    offline: bool,
}
impl Options {
    fn parse(args: &[String]) -> AnyResult<Self> {
        let mut out = Self {
            args: Vec::new(),
            values: BTreeMap::new(),
            json: false,
            offline: false,
        };
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--json" => out.json = true,
                "--offline" => out.offline = true,
                "--backend" | "--scenario" | "--trust-file" | "--trusted-issuer"
                | "--trusted-order" | "--trusted_issuer" | "--trusted_order" | "--environment"
                | "--output-dir" | "--endpoint" | "--key-file" => {
                    let value = it
                        .next()
                        .filter(|v| !v.starts_with("--"))
                        .ok_or("option requires a value")?;
                    if out
                        .values
                        .insert(arg.replace('_', "-"), value.clone())
                        .is_some()
                    {
                        return Err("duplicate option".into());
                    }
                }
                other if other.starts_with('-') => {
                    return Err(format!("unknown option {other}").into())
                }
                _ => out.args.push(arg.clone()),
            }
        }
        Ok(out)
    }
    fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }
    fn trust(&self) -> AnyResult<ReceiptTrust> {
        let mut value = if let Some(file) = self.get("--trust-file") {
            serde_json::from_slice(&fs::read(file)?)?
        } else {
            json!({})
        };
        for (option, key) in [
            ("--trusted-order", "trusted_order"),
            ("--trusted-issuer", "trusted_issuer"),
            ("--environment", "environment"),
        ] {
            if let Some(v) = self.get(option) {
                value[key] = v.into();
            }
        }
        let t: ReceiptTrust = serde_json::from_value(value)?;
        hex::<32>(&t.trusted_issuer)?;
        hex::<32>(&t.trusted_order)?;
        environment(&t.environment)?;
        Ok(t)
    }
}
fn print_value(v: &Value, keys: &[&str], json: bool) -> AnyResult<()> {
    if json {
        println!("{}", serde_json::to_string(v)?);
    } else {
        for &k in keys {
            match &v[k] {
                Value::String(s) => println!("{k}={s}"),
                val => println!("{k}={val}"),
            }
        }
    }
    Ok(())
}
fn inspect(opts: &Options, order: &str) -> AnyResult<u8> {
    let trust = opts.trust()?;
    if order != trust.trusted_order {
        return Ok(3);
    }
    let dir = opts
        .get("--trust-file")
        .and_then(|p| Path::new(p).parent())
        .unwrap_or(Path::new("."));
    let default_key = dir.join("buyer.key");
    let key = session::read_key(
        opts.get("--key-file")
            .map(Path::new)
            .unwrap_or(&default_key),
    )?;
    let server;
    let endpoint = if let Some(url) = opts.get("--endpoint") {
        url
    } else {
        server = session::Server::start(&dir.join("coordinator.json"))?;
        &server.base
    };
    let http = HttpCoordinator::new(endpoint, &trust.environment, key, Role::Buyer)?;
    let projection = http.order(order)?;
    let review = &projection["review"];
    if review["order_id"] != trust.trusted_order
        || review["receipt_issuer"] != trust.trusted_issuer
        || review["environment"] != trust.environment
    {
        return Ok(3);
    }
    let mut result = json!({"order_id":order});
    let balances = [
        "state",
        "held",
        "paid",
        "refunded",
        "buyer_balance",
        "seller_balance",
        "delivery_id",
        "verdict_id",
    ];
    let terms = ["deadlines", "request_id", "quote_id", "acceptance_id"];
    for key in balances {
        result[key] = projection[key].clone();
    }
    for key in terms {
        result[key] = review[key].clone();
    }
    let mut keys = vec!["order_id"];
    keys.extend(balances);
    keys.extend(terms);
    print_value(&result, &keys, opts.json)?;
    Ok(0)
}
fn run(args: &[String]) -> AnyResult<u8> {
    if args.first().is_some_and(|v| v == "__serve") {
        settle463d::run_cli(&args[1..])?;
        return Ok(0);
    }
    let opts = Options::parse(args)?;
    let cmd: Vec<_> = opts.args.iter().map(String::as_str).collect();
    match cmd.as_slice() {
        ["demo"] if opts.get("--backend")==Some("sim") => {
            let scenario=opts.get("--scenario").ok_or("missing --scenario")?;
            let dir=session::create_dir(opts.get("--output-dir"))?;
            let result=demo::run(scenario,&dir)?;
            let keys=["order_id","backend","outcome","asset","principal","delta_buyer","delta_seller","delta_escrow","receipt"];
            print_value(&result,&keys,opts.json)?;
            Ok(0)
        }
        ["receipt","verify",bundle] if opts.offline => {
            let trust=opts.trust()?;
            let bytes=fs::read(bundle)?;
            let (code,label)=match verify_sim_bundle(&bytes,&trust) {
                Ok(_)=>(0,"evidence_signed"),
                Err(ReceiptError::InvalidEvidence)=>(2,"invalid_evidence"),
                Err(ReceiptError::Untrusted)=>(3,"untrusted"),
            };
            if opts.json {println!("{}",json!({"result":label}));} else {println!("{label}");}
            Ok(code)
        }
        ["order","inspect",order] => inspect(&opts,order),
        _ => Err("usage: settle463 demo --backend sim --scenario <scenario> | receipt verify <bundle> --offline --trust-file <file> | order inspect <id> --trust-file <file>".into()),
    }
}
fn main() -> ExitCode {
    match run(&std::env::args().skip(1).collect::<Vec<_>>()) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
