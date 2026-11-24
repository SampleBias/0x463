//! Encodings used by signed messages and ledger locators.

use crate::{Error, Result};

pub fn hex<const N: usize>(value: &str) -> Result<[u8; N]> {
    fn digit(c: u8) -> Result<u8> {
        match c {
            b'0'..=b'9' => Ok(c - b'0'),
            b'a'..=b'f' => Ok(c - b'a' + 10),
            _ => Err(Error::Invalid("lowercase hex")),
        }
    }
    if value.len() != N * 2 {
        return Err(Error::Invalid("hex width"));
    }
    let mut out = [0; N];
    for (dst, pair) in out.iter_mut().zip(value.as_bytes().chunks_exact(2)) {
        *dst = (digit(pair[0])? << 4) | digit(pair[1])?;
    }
    Ok(out)
}

pub fn hex_string(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(DIGITS[(b >> 4) as usize] as char);
        out.push(DIGITS[(b & 15) as usize] as char);
    }
    out
}

pub fn decimal(s: &str) -> Result<u64> {
    if s.is_empty() || (s.len() > 1 && s.starts_with('0')) || !s.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(Error::Invalid("decimal string"));
    }
    s.parse().map_err(|_| Error::Invalid("u64 range"))
}

pub fn timestamp(s: &str) -> Result<i64> {
    i64::try_from(decimal(s)?).map_err(|_| Error::Invalid("timestamp range"))
}

pub fn base58<const N: usize>(s: &str) -> Result<[u8; N]> {
    const DIGITS: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    if s.is_empty() || s.len() > N * 2 {
        return Err(Error::Invalid("base58 width"));
    }
    let mut bytes = [0; N];
    for c in s.bytes() {
        let mut carry = DIGITS
            .iter()
            .position(|&d| d == c)
            .ok_or(Error::Invalid("base58 digit"))? as u16;
        for b in bytes.iter_mut().rev() {
            carry += u16::from(*b) * 58;
            *b = carry as u8;
            carry >>= 8;
        }
        if carry != 0 {
            return Err(Error::Invalid("base58 overflow"));
        }
    }
    // Each leading zero byte has exactly one leading '1' in canonical base58.
    if s.bytes().take_while(|&c| c == b'1').count() != bytes.iter().take_while(|&&b| b == 0).count()
    {
        return Err(Error::Invalid("base58 width"));
    }
    Ok(bytes)
}

pub fn environment(s: &str) -> Result<()> {
    if let Some(id) = s.strip_prefix("sim:") {
        if (1..=64).contains(&id.len())
            && !id.starts_with('-')
            && !id.ends_with('-')
            && id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Ok(());
        }
    } else if let Some(rest) = s.strip_prefix("solana:") {
        let mut fields = rest.split(':');
        for _ in 0..3 {
            base58::<32>(fields.next().unwrap_or(""))?;
        }
        if fields.next().is_none() {
            return Ok(());
        }
    }
    Err(Error::Invalid("environment"))
}
