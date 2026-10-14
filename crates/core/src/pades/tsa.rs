//! Sello de tiempo (RFC 3161) para la firma: un servidor de sellado (TSA)
//! firma la hora junto con el resumen de nuestra firma. Así la hora de la
//! firma no es solo la del reloj del equipo (PAdES B-T).
//!
//! Solo servidores `http://` (el sello va firmado por el servidor, no hace
//! falta TLS), con un cliente HTTP mínimo para no añadir dependencias.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use sha2::{Digest, Sha256};

/// Servidor de sellado propuesto: el de DigiCert, gratuito y de confianza
/// en Adobe.
pub const DEFAULT_TSA: &str = "http://timestamp.digicert.com";

/// Pide un sello de tiempo para `data` (el valor de la firma). Devuelve el
/// TimeStampToken (un ContentInfo DER).
pub fn timestamp(url: &str, data: &[u8]) -> Result<Vec<u8>> {
    let digest = Sha256::digest(data);
    let req = request(&digest);
    let body = post(url, &req).with_context(|| crate::t!("pades.tsa_failed", url))?;
    parse_response(&body).with_context(|| crate::t!("pades.tsa_failed", url))
}

/// TimeStampReq con SHA-256, un número aleatorio y pidiendo el certificado.
fn request(digest: &[u8]) -> Vec<u8> {
    let nonce: [u8; 8] = {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        ((t as u64) ^ ((std::process::id() as u64) << 32)).to_be_bytes()
    };
    let mut imprint = vec![0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01, 0x05, 0x00];
    imprint.extend(tlv(0x04, digest));
    let mut nonce_int = Vec::new();
    if nonce[0] & 0x80 != 0 {
        nonce_int.push(0);
    }
    nonce_int.extend_from_slice(&nonce);
    let mut body = vec![0x02, 0x01, 0x01];
    body.extend(tlv(0x30, &imprint));
    body.extend(tlv(0x02, &nonce_int));
    body.extend([0x01, 0x01, 0xff]);
    tlv(0x30, &body)
}

fn tlv(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let n = content.len();
    if n < 0x80 {
        out.push(n as u8);
    } else {
        let bytes: Vec<u8> = (n as u32).to_be_bytes().into_iter().skip_while(|&b| b == 0).collect();
        out.push(0x80 | bytes.len() as u8);
        out.extend(bytes);
    }
    out.extend_from_slice(content);
    out
}

/// Lee un TLV: (etiqueta, contenido, resto).
fn read_tlv(b: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let tag = *b.first()?;
    let first = *b.get(1)? as usize;
    let (len, start) = if first < 0x80 {
        (first, 2)
    } else {
        let n = first & 0x7f;
        if n == 0 || n > 4 {
            return None;
        }
        let len = b.get(2..2 + n)?.iter().fold(0usize, |a, &x| (a << 8) | x as usize);
        (len, 2 + n)
    };
    let content = b.get(start..start + len)?;
    Some((tag, content, &b[start + len..]))
}

/// De la respuesta (TimeStampResp), el sello, si el servidor lo concedió.
fn parse_response(b: &[u8]) -> Result<Vec<u8>> {
    let (tag, resp, _) = read_tlv(b).ok_or_else(|| anyhow!("respuesta no válida"))?;
    if tag != 0x30 {
        bail!("respuesta no válida");
    }
    let (_, status_info, rest) = read_tlv(resp).ok_or_else(|| anyhow!("respuesta no válida"))?;
    let (_, status, _) = read_tlv(status_info).ok_or_else(|| anyhow!("respuesta no válida"))?;
    // 0: concedido; 1: concedido con cambios.
    let code = status.iter().fold(0u32, |a, &x| (a << 8) | x as u32);
    if code > 1 {
        bail!("{}", crate::t!("pades.tsa_rejected", code));
    }
    let (tag, _, _) = read_tlv(rest).ok_or_else(|| anyhow!("respuesta sin sello"))?;
    if tag != 0x30 {
        bail!("respuesta sin sello");
    }
    // El ContentInfo entero (con su cabecera).
    let token_len = rest.len() - read_tlv(rest).map_or(0, |(_, _, r)| r.len());
    Ok(rest[..token_len].to_vec())
}

/// POST HTTP/1.1 sencillo. Devuelve el cuerpo de la respuesta.
fn post(url: &str, body: &[u8]) -> Result<Vec<u8>> {
    let Some(rest) = url.strip_prefix("http://") else {
        bail!("{}", crate::t!("pades.tsa_http_only"));
    };
    let (host_port, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match host_port.rsplit_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().unwrap_or(80)),
        None => (host_port, 80),
    };
    let addr = (host, port).to_socket_addrs()?.next().ok_or_else(|| anyhow!("{host}: sin dirección"))?;
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(15))?;
    s.set_read_timeout(Some(Duration::from_secs(30)))?;
    s.set_write_timeout(Some(Duration::from_secs(30)))?;
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: {host_port}\r\nUser-Agent: Form PDF Reader/{}\r\nContent-Type: application/timestamp-query\r\nAccept: application/timestamp-reply\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        env!("CARGO_PKG_VERSION"),
        body.len()
    );
    s.write_all(head.as_bytes())?;
    s.write_all(body)?;
    let mut resp = Vec::new();
    s.take(4 << 20).read_to_end(&mut resp)?;
    let split = resp.windows(4).position(|w| w == b"\r\n\r\n").ok_or_else(|| anyhow!("respuesta HTTP incompleta"))?;
    let headers = String::from_utf8_lossy(&resp[..split]).to_ascii_lowercase();
    let status = headers.split_whitespace().nth(1).unwrap_or("");
    if status != "200" {
        bail!("HTTP {status}");
    }
    let body = &resp[split + 4..];
    if headers.contains("transfer-encoding: chunked") {
        return dechunk(body);
    }
    Ok(body.to_vec())
}

fn dechunk(mut b: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let line_end = b.windows(2).position(|w| w == b"\r\n").ok_or_else(|| anyhow!("trozo HTTP incompleto"))?;
        let size_str = String::from_utf8_lossy(&b[..line_end]);
        let size = usize::from_str_radix(size_str.split(';').next().unwrap_or("").trim(), 16)?;
        b = &b[line_end + 2..];
        if size == 0 {
            return Ok(out);
        }
        out.extend_from_slice(b.get(..size).ok_or_else(|| anyhow!("trozo HTTP incompleto"))?);
        b = b.get(size + 2..).unwrap_or(&[]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peticion_y_respuesta() {
        let r = request(&[7u8; 32]);
        assert_eq!(r[0], 0x30);
        let (_, body, rest) = read_tlv(&r).unwrap();
        assert!(rest.is_empty());
        assert_eq!(&body[..3], &[0x02, 0x01, 0x01]);
        // Respuesta concedida con un "sello" de mentira.
        let token = tlv(0x30, &[0x06, 0x01, 0x2a]);
        let mut resp_body = tlv(0x30, &[0x02, 0x01, 0x00]);
        resp_body.extend(&token);
        assert_eq!(parse_response(&tlv(0x30, &resp_body)).unwrap(), token);
        let rejected = tlv(0x30, &tlv(0x30, &[0x02, 0x01, 0x02]));
        assert!(parse_response(&rejected).is_err());
        assert_eq!(dechunk(b"3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n").unwrap(), b"abcde");
    }
}
