//! One HTTP/1.1 request over a stream the transport opened, for the relay's group calls. The
//! connection closes after the answer, so the body ends where the stream does.
//! Required Notice: Copyright (c) 2026 Alazy Inc. (https://alazy.app)

use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// What a call may read of the answer; the relay's are a few hundred bytes.
const MAX_ANSWER: usize = 1024 * 1024;

pub(crate) struct Answer {
    pub status: u16,
    pub retry_after: Option<String>,
    pub reason: Option<String>,
    pub body: Vec<u8>,
}

fn bad(why: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, why.to_string())
}

pub(crate) async fn request(
    mut stream: impl AsyncRead + AsyncWrite + Unpin,
    host: &str,
    method: &str,
    path: &str,
    json: Option<&str>,
    bearer: Option<&str>,
) -> io::Result<Answer> {
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nAccept: application/json\r\n"
    );
    if let Some(token) = bearer {
        head += &format!("Authorization: Bearer {token}\r\n");
    }
    if let Some(body) = json {
        head += &format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            body.len()
        );
    }
    head += "\r\n";
    stream.write_all(head.as_bytes()).await?;
    if let Some(body) = json {
        stream.write_all(body.as_bytes()).await?;
    }
    stream.flush().await?;

    let mut raw = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&chunk[..n]);
        if raw.len() > MAX_ANSWER {
            return Err(bad("the answer is too large"));
        }
    }

    let mut headers = [httparse::EMPTY_HEADER; 32];
    let mut res = httparse::Response::new(&mut headers);
    let httparse::Status::Complete(head_len) = res.parse(&raw).map_err(|e| bad(&e.to_string()))?
    else {
        return Err(bad("the connection ended inside the headers"));
    };
    let status = res.code.ok_or_else(|| bad("no status"))?;
    let header = |name: &str| {
        res.headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(name))
            .map(|h| String::from_utf8_lossy(h.value).trim().to_string())
    };
    let rest = &raw[head_len..];
    let body = if header("transfer-encoding").is_some_and(|v| v.eq_ignore_ascii_case("chunked")) {
        dechunk(rest)?
    } else if let Some(len) = header("content-length").and_then(|v| v.parse::<usize>().ok()) {
        rest.get(..len)
            .ok_or_else(|| bad("the connection ended inside the body"))?
            .to_vec()
    } else {
        rest.to_vec()
    };
    Ok(Answer {
        status,
        retry_after: header("retry-after"),
        reason: header("x-relay-reason"),
        body,
    })
}

fn dechunk(mut rest: &[u8]) -> io::Result<Vec<u8>> {
    let mut body = Vec::new();
    loop {
        let line_end = rest
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or_else(|| bad("the connection ended inside a chunk"))?;
        let size = std::str::from_utf8(&rest[..line_end])
            .ok()
            .and_then(|l| usize::from_str_radix(l.split(';').next()?.trim(), 16).ok())
            .ok_or_else(|| bad("bad chunk size"))?;
        rest = &rest[line_end + 2..];
        if size == 0 {
            return Ok(body);
        }
        if rest.len() < size + 2 {
            return Err(bad("the connection ended inside a chunk"));
        }
        body.extend_from_slice(&rest[..size]);
        rest = &rest[size + 2..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn ask(reply: &'static [u8]) -> io::Result<Answer> {
        let (client, mut server) = tokio::io::duplex(1024);
        tokio::spawn(async move {
            let mut buf = [0u8; 1024];
            let _ = server.read(&mut buf).await;
            server.write_all(reply).await.unwrap();
            server.shutdown().await.unwrap();
        });
        request(client, "relay.test", "GET", "/x", None, None).await
    }

    #[tokio::test]
    async fn reads_a_length_delimited_body() {
        let a = ask(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nRetry-After: 7\r\n\r\nhello")
            .await
            .unwrap();
        assert_eq!(
            (a.status, a.body, a.retry_after.as_deref()),
            (200, b"hello".to_vec(), Some("7"))
        );
    }

    #[tokio::test]
    async fn reads_chunks_and_a_body_that_ends_with_the_connection() {
        let a = ask(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2;x=1\r\nde\r\n0\r\n\r\n").await.unwrap();
        assert_eq!(a.body, b"abcde");
        let a = ask(b"HTTP/1.1 503 Busy\r\n\r\nrest").await.unwrap();
        assert_eq!((a.status, a.body), (503, b"rest".to_vec()));
    }

    #[tokio::test]
    async fn refuses_a_cut_off_answer() {
        assert!(
            ask(b"HTTP/1.1 200 OK\r\nContent-Length: 50\r\n\r\nshort")
                .await
                .is_err()
        );
        assert!(
            ask(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nab")
                .await
                .is_err()
        );
        assert!(ask(b"HTTP/1.1 200 OK\r\nCont").await.is_err());
        assert!(ask(b"garbage").await.is_err());
    }

    #[tokio::test]
    async fn sends_the_body_with_its_length() {
        let (client, mut server) = tokio::io::duplex(1024);
        let seen = tokio::spawn(async move {
            let mut buf = vec![0u8; 1024];
            let n = server.read(&mut buf).await.unwrap();
            server
                .write_all(b"HTTP/1.1 204 No Content\r\n\r\n")
                .await
                .unwrap();
            server.shutdown().await.unwrap();
            String::from_utf8(buf[..n].to_vec()).unwrap()
        });
        let a = request(client, "relay.test", "POST", "/p", Some("{\"a\":1}"), None)
            .await
            .unwrap();
        assert_eq!(a.status, 204);
        let sent = seen.await.unwrap();
        assert!(sent.starts_with("POST /p HTTP/1.1\r\nHost: relay.test\r\n"));
        assert!(sent.contains("Content-Length: 7\r\n"));
        assert!(sent.ends_with("\r\n\r\n{\"a\":1}"));
    }
}
