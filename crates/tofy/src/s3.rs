//! Minimal S3-compatible CreateBucket / HeadBucket against the local object store.
//! Used after the container is up so `TOFY_*_BUCKET` names a real bucket.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const REGION: &str = "us-east-1";

type HmacSha256 = Hmac<Sha256>;

pub fn wait_for_object_store(port: u16) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if live(port) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(Error::Engine(format!(
                "object store on 127.0.0.1:{port} did not accept connections within 60s"
            )));
        }
        std::thread::sleep(Duration::from_millis(400));
    }
}

pub fn ensure_bucket(port: u16, access_key: &str, secret_key: &str, bucket: &str) -> Result<()> {
    match signed_status("HEAD", port, bucket, access_key, secret_key)? {
        200 | 204 => return Ok(()),
        404 | 0 => {}
        other => {
            return Err(Error::Engine(format!(
                "HEAD bucket {bucket} on 127.0.0.1:{port} returned {other}"
            )));
        }
    }
    match signed_status("PUT", port, bucket, access_key, secret_key)? {
        200 | 204 | 409 => Ok(()),
        status => Err(Error::Engine(format!(
            "failed to create bucket {bucket} on 127.0.0.1:{port}: HTTP {status}"
        ))),
    }
}

fn live(port: u16) -> bool {
    let req = format!(
        "GET /minio/health/live HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    );
    matches!(http_exchange(port, req.as_bytes()), Ok((200, _)))
}

fn signed_status(
    method: &str,
    port: u16,
    bucket: &str,
    access_key: &str,
    secret_key: &str,
) -> Result<u16> {
    let (amz_date, datestamp) = amz_now();
    let host = format!("127.0.0.1:{port}");
    let path = format!("/{bucket}");
    let auth = authorization(
        method, &path, &host, access_key, secret_key, &amz_date, &datestamp,
    );
    let req = format!(
        "{method} {path} HTTP/1.1\r\n\
         Host: {host}\r\n\
         x-amz-content-sha256: {EMPTY_SHA256}\r\n\
         x-amz-date: {amz_date}\r\n\
         Authorization: {auth}\r\n\
         Connection: close\r\n\
         Content-Length: 0\r\n\
         \r\n"
    );
    let (status, _) = http_exchange(port, req.as_bytes())?;
    Ok(status)
}

fn authorization(
    method: &str,
    path: &str,
    host: &str,
    access_key: &str,
    secret_key: &str,
    amz_date: &str,
    datestamp: &str,
) -> String {
    let canonical_headers =
        format!("host:{host}\nx-amz-content-sha256:{EMPTY_SHA256}\nx-amz-date:{amz_date}\n");
    let signed_headers = "host;x-amz-content-sha256;x-amz-date";
    let canonical_request =
        format!("{method}\n{path}\n\n{canonical_headers}\n{signed_headers}\n{EMPTY_SHA256}");
    let scope = format!("{datestamp}/{REGION}/s3/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        hex_sha256(canonical_request.as_bytes())
    );
    let signature = hex::encode(hmac_sha256(
        &signing_key(secret_key, datestamp),
        string_to_sign.as_bytes(),
    ));
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key}/{scope}, SignedHeaders={signed_headers}, Signature={signature}"
    )
}

fn signing_key(secret: &str, datestamp: &str) -> Vec<u8> {
    let k_date = hmac_sha256(format!("AWS4{secret}").as_bytes(), datestamp.as_bytes());
    let k_region = hmac_sha256(&k_date, REGION.as_bytes());
    let k_service = hmac_sha256(&k_region, b"s3");
    hmac_sha256(&k_service, b"aws4_request")
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("hmac key");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn hex_sha256(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

fn http_exchange(port: u16, request: &[u8]) -> Result<(u16, String)> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))
        .map_err(|e| Error::Engine(format!("object store connect 127.0.0.1:{port}: {e}")))?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(2))).ok();
    stream.write_all(request)?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf)?;
    let text = String::from_utf8_lossy(&buf).into_owned();
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    Ok((status, text))
}

fn amz_now() -> (String, String) {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (year, month, day, hour, min, sec) = unix_to_utc(secs);
    (
        format!("{year:04}{month:02}{day:02}T{hour:02}{min:02}{sec:02}Z"),
        format!("{year:04}{month:02}{day:02}"),
    )
}

/// Howard Hinnant's civil-from-days, UTC.
fn unix_to_utc(secs: u64) -> (i32, u32, u32, u32, u32, u32) {
    let z = (secs / 86400) as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = (y + i64::from(m <= 2)) as i32;
    let rem = secs % 86400;
    (
        year,
        m,
        d,
        (rem / 3600) as u32,
        ((rem % 3600) / 60) as u32,
        (rem % 60) as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_payload_hash_is_aws_constant() {
        assert_eq!(hex_sha256(b""), EMPTY_SHA256);
    }

    #[test]
    fn unix_epoch_is_1970_01_01() {
        assert_eq!(unix_to_utc(0), (1970, 1, 1, 0, 0, 0));
        assert_eq!(unix_to_utc(1_776_816_000), (2026, 4, 22, 0, 0, 0));
    }

    #[test]
    fn authorization_is_stable_for_fixed_clock() {
        let auth = authorization(
            "PUT",
            "/uploads",
            "127.0.0.1:9000",
            "AKID",
            "secret",
            "20260422T000000Z",
            "20260422",
        );
        assert!(
            auth.starts_with("AWS4-HMAC-SHA256 Credential=AKID/20260422/us-east-1/s3/aws4_request")
        );
        assert!(auth.contains("Signature="));
        assert_eq!(auth.len(), auth.chars().count());
    }
}
