//! A persistent demo directory lets a fresh inspect process reopen the same coordinator.

use crate::AnyResult;
use settle463_core::{
    ident::{hex, hex_string},
    receipt::ReceiptTrust,
};
use settle463_sdk::Identity;
use std::{
    env, fs,
    io::{BufRead, BufReader},
    net::SocketAddr,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
};

pub fn random() -> AnyResult<[u8; 32]> {
    let mut bytes = [0; 32];
    getrandom::getrandom(&mut bytes).map_err(|e| format!("randomness unavailable: {e}"))?;
    Ok(bytes)
}
pub fn private_write(path: &Path, data: &[u8]) -> AnyResult<()> {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(data)?;
    Ok(())
}
pub fn create_dir(opt: Option<&str>) -> AnyResult<PathBuf> {
    let dir = match opt {
        Some(p) => PathBuf::from(p),
        None => env::temp_dir().join(format!("settle463-{}", hex_string(&random()?[..16]))),
    };
    fs::DirBuilder::new().mode(0o700).create(&dir)?;
    Ok(dir.canonicalize()?)
}
pub fn key(dir: &Path, name: &str) -> AnyResult<Identity> {
    let seed = random()?;
    private_write(
        &dir.join(format!("{name}.key")),
        hex_string(&seed).as_bytes(),
    )?;
    Ok(Identity::from_seed(&seed))
}
pub fn read_key(path: &Path) -> AnyResult<Identity> {
    Ok(Identity::from_seed(&hex(fs::read_to_string(path)?.trim())?))
}
pub struct Server {
    child: Child,
    pub base: String,
}
impl Server {
    pub fn start(config: &Path) -> AnyResult<Self> {
        let exe = env::current_exe()?;
        // A clean cargo run builds the CLI and links the same daemon entry point as settle463d.
        let mut child = Command::new(exe)
            .arg("__serve")
            .arg("--config")
            .arg(config)
            .args(["--listen", "127.0.0.1:0"])
            .stdout(Stdio::piped())
            .spawn()?;
        let ready = (|| -> AnyResult<String> {
            let mut line = String::new();
            BufReader::new(child.stdout.take().ok_or("daemon stdout unavailable")?)
                .read_line(&mut line)?;
            let addr: SocketAddr = line
                .trim()
                .strip_prefix("LISTEN=")
                .ok_or("coordinator startup failed")?
                .parse()?;
            if !addr.ip().is_loopback() || addr.port() == 0 {
                return Err("invalid coordinator endpoint".into());
            }
            Ok(format!("http://{addr}"))
        })();
        match ready {
            Ok(base) => Ok(Self { child, base }),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                Err(e)
            }
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
pub fn write_trust(dir: &Path, trust: &ReceiptTrust) -> AnyResult<()> {
    private_write(&dir.join("trust.json"), &serde_json::to_vec_pretty(trust)?)
}
