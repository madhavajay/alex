//! Reverse wrap: log + forward HTTP/WS to an upstream (http or https).
//!
//! Handles HTTP/1.1 keep-alive by processing request/response cycles on a
//! connection until an Upgrade (WebSocket) switches to a full byte tunnel.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine;

use anyhow::{anyhow, bail, Context, Result};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio_rustls::rustls::{pki_types::ServerName, ClientConfig, RootCertStore};
use tokio_rustls::TlsConnector;

use crate::capture::{CaptureEvent, CaptureLog};
use crate::catalog::WrapReverseInject;

/// Running reverse wrap server.
pub struct ReverseWrap {
    pub listen_addr: SocketAddr,
    pub log: CaptureLog,
    pub upstream: String,
    shutdown: Option<oneshot::Sender<()>>,
    join: Option<tokio::task::JoinHandle<()>>,
}

#[derive(Clone)]
struct UpstreamTarget {
    display: String,
    host: String,
    port: u16,
    tls: bool,
}

/// Optional request rewrites for reverse wrap (catalog-driven).
#[derive(Clone, Default)]
pub struct ReverseOptions {
    pub inject: Option<WrapReverseInject>,
}

impl UpstreamTarget {
    fn parse(url: &str) -> Result<Self> {
        let url = url.trim().trim_end_matches('/');
        let (tls, rest) = if let Some(r) = url.strip_prefix("https://") {
            (true, r)
        } else if let Some(r) = url.strip_prefix("http://") {
            (false, r)
        } else {
            (true, url)
        };
        let (host_port, _) = rest.split_once('/').unwrap_or((rest, ""));
        let (host, port) = if let Some((h, p)) = host_port.rsplit_once(':') {
            if h.contains(']') {
                (host_port.to_string(), if tls { 443 } else { 80 })
            } else {
                (
                    h.to_string(),
                    p.parse::<u16>()
                        .with_context(|| format!("bad port in upstream {url}"))?,
                )
            }
        } else {
            (host_port.to_string(), if tls { 443 } else { 80 })
        };
        if host.is_empty() {
            bail!("empty upstream host in {url}");
        }
        Ok(Self {
            display: url.to_string(),
            host,
            port,
            tls,
        })
    }
}

impl ReverseWrap {
    pub async fn start_http_to_http(
        bind: SocketAddr,
        upstream: SocketAddr,
        log: CaptureLog,
    ) -> Result<Self> {
        let target = UpstreamTarget {
            display: format!("http://{upstream}"),
            host: upstream.ip().to_string(),
            port: upstream.port(),
            tls: false,
        };
        Self::start(bind, target, log, ReverseOptions::default()).await
    }

    pub async fn start_to_url(
        bind: SocketAddr,
        upstream_url: &str,
        log: CaptureLog,
    ) -> Result<Self> {
        Self::start_to_url_with(bind, upstream_url, log, ReverseOptions::default()).await
    }

    pub async fn start_to_url_with(
        bind: SocketAddr,
        upstream_url: &str,
        log: CaptureLog,
        opts: ReverseOptions,
    ) -> Result<Self> {
        let target = UpstreamTarget::parse(upstream_url)?;
        Self::start(bind, target, log, opts).await
    }

    async fn start(
        bind: SocketAddr,
        target: UpstreamTarget,
        log: CaptureLog,
        opts: ReverseOptions,
    ) -> Result<Self> {
        let listener = TcpListener::bind(bind).await?;
        let listen_addr = listener.local_addr()?;
        let (tx, mut rx) = oneshot::channel::<()>();
        let log_c = log.clone();
        let upstream_display = target.display.clone();
        let join = tokio::spawn(async move {
            let tls = if target.tls {
                Some(Arc::new(build_tls_connector()))
            } else {
                None
            };
            let inject = opts.inject;
            loop {
                tokio::select! {
                    _ = &mut rx => break,
                    accept = listener.accept() => {
                        match accept {
                            Ok((stream, _)) => {
                                let log = log_c.clone();
                                let target = target.clone();
                                let tls = tls.clone();
                                let inject = inject.clone();
                                tokio::spawn(async move {
                                    if let Err(e) =
                                        handle_client(stream, target.clone(), tls, log.clone(), inject).await
                                    {
                                        // Connection churn is normal for interactive harnesses.
                                        // Always retain the diagnostic in flows.jsonl, but do not
                                        // let the default WARN subscriber paint over their TUI.
                                        // Set ALEX_WRAP_DEBUG_ERRORS=1 for terminal diagnostics.
                                        log.push(CaptureEvent {
                                            seq: 0,
                                            kind: "error".into(),
                                            method: None,
                                            path: None,
                                            host: Some(target.display.clone()),
                                            status: None,
                                            body_len: None,
                                            note: Some(format!("{e:#}")),
                                        });
                                        if std::env::var_os("ALEX_WRAP_DEBUG_ERRORS").is_some() {
                                            eprintln!("alex wrap: client error: {e:#}");
                                        }
                                    }
                                });
                            }
                            Err(e) => tracing::debug!("wrap accept error: {e}"),
                        }
                    }
                }
            }
        });
        Ok(Self {
            listen_addr,
            log,
            upstream: upstream_display,
            shutdown: Some(tx),
            join: Some(join),
        })
    }

    pub fn base_url(&self) -> String {
        format!("http://{}", self.listen_addr)
    }

    pub async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(join) = self.join.take() {
            let _ = join.await;
        }
    }
}

fn ensure_crypto_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

fn build_tls_connector() -> TlsConnector {
    ensure_crypto_provider();
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let mut cfg = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    // The reverse wrap is a raw HTTP/1.1 proxy. Do not negotiate h2 with
    // upstream or the server will expect HTTP/2 frames while we forward an
    // HTTP/1.1 request head from the harness.
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    TlsConnector::from(Arc::new(cfg))
}

async fn handle_client(
    mut client: TcpStream,
    target: UpstreamTarget,
    tls: Option<Arc<TlsConnector>>,
    log: CaptureLog,
    inject: Option<WrapReverseInject>,
) -> Result<()> {
    client.set_nodelay(true).ok();

    // Read the first request *before* dialing upstream so a slow/blocked
    // upstream connect does not strand an unread client socket (CLOSE_WAIT).
    let first = match read_http_headers(&mut client).await? {
        Some(h) => h,
        None => return Ok(()),
    };

    let tcp = dial_upstream(&target).await?;
    tcp.set_nodelay(true).ok();

    if let Some(connector) = tls {
        let server_name = ServerName::try_from(target.host.clone())
            .map_err(|_| anyhow!("invalid TLS server name {}", target.host))?;
        let peer = tcp.peer_addr().ok();
        let up = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            connector.connect(server_name, tcp),
        )
        .await
        .with_context(|| format!("timeout TLS handshake to upstream {peer:?}"))?
        .with_context(|| format!("TLS handshake to upstream {peer:?}"))?;
        proxy_http_cycles(&mut client, up, &target, &log, inject.as_ref(), Some(first)).await
    } else {
        proxy_http_cycles(
            &mut client,
            tcp,
            &target,
            &log,
            inject.as_ref(),
            Some(first),
        )
        .await
    }
}

/// Read request(s) from client, forward, return response(s). On WebSocket upgrade,
/// switch to raw bidirectional tunnel for the rest of the connection.
///
/// `prefetched` is the first request head already read before dialing upstream
/// (avoids head-of-line hang when upstream is slow).
async fn proxy_http_cycles<U>(
    client: &mut TcpStream,
    mut up: U,
    target: &UpstreamTarget,
    log: &CaptureLog,
    inject: Option<&WrapReverseInject>,
    mut prefetched: Option<Vec<u8>>,
) -> Result<()>
where
    U: AsyncRead + AsyncWrite + Unpin,
{
    loop {
        let head = if let Some(h) = prefetched.take() {
            h
        } else {
            match read_http_headers(client).await? {
                Some(h) => h,
                None => return Ok(()), // client closed
            }
        };

        // body_start = index just after the header block's terminating \r\n\r\n
        let (req_line, body_start) =
            split_headers(&head).ok_or_else(|| anyhow!("bad request headers"))?;
        let parts: Vec<&str> = req_line.split_whitespace().collect();
        if parts.len() < 2 {
            bail!("bad request line");
        }
        let method = parts[0].to_string();
        let path = parts[1].to_string();
        let header_text = std::str::from_utf8(&head[..body_start.min(head.len())]).unwrap_or("");
        let is_upgrade = header_text
            .to_ascii_lowercase()
            .contains("upgrade: websocket");
        let content_length = parse_content_length(header_text);
        let is_chunked = header_text
            .to_ascii_lowercase()
            .contains("transfer-encoding: chunked");

        log.push(CaptureEvent {
            seq: 0,
            kind: if is_upgrade {
                "ws_upgrade".into()
            } else {
                "http_req".into()
            },
            method: Some(method.clone()),
            path: Some(path.clone()),
            host: Some(target.display.clone()),
            status: None,
            body_len: content_length,
            note: None,
        });

        if std::env::var_os("ALEX_WRAP_DUMP_HEADERS").is_some() {
            dump_headers(&method, &path, &head);
        }

        // rewrite includes any body bytes that arrived with the header packet
        let rewritten =
            rewrite_request_headers(&head, &target.host, target.port, target.tls, inject)?;
        up.write_all(&rewritten).await?;

        // Remaining body still on the client stream (not yet in `head`)
        let already = head.len().saturating_sub(body_start);
        let mut req_capture = Vec::new();
        capture_extend(&mut req_capture, &head[body_start..]);
        if is_chunked {
            if already == 0 {
                copy_chunked_body_capture(client, &mut up, &mut req_capture).await?;
            } else {
                copy_chunked_body_remaining_capture(
                    client,
                    &mut up,
                    &head[body_start..],
                    &mut req_capture,
                )
                .await?;
            }
        } else if let Some(cl) = content_length {
            copy_fixed_body_capture(
                client,
                &mut up,
                cl.saturating_sub(already),
                &mut req_capture,
            )
            .await?;
        }
        append_http_body_capture(
            log,
            "http_req_body",
            &method,
            &path,
            &req_capture,
            already,
            is_chunked,
        );
        up.flush().await.ok();

        if is_upgrade {
            // After 101, raw tunnel both ways for the rest of the connection.
            let resp_head = read_http_headers(&mut up)
                .await?
                .ok_or_else(|| anyhow!("upstream closed during upgrade"))?;
            let status = parse_status_from_headers(&resp_head);
            log.push(CaptureEvent {
                seq: 0,
                kind: "http_resp".into(),
                method: Some(method),
                path: Some(path),
                host: Some(target.display.clone()),
                status,
                body_len: None,
                note: Some("websocket upgrade".into()),
            });
            client.write_all(&resp_head).await?;
            // Any body after 101 headers is already part of the tunnel stream —
            // resp_head includes only headers; remainder stays in up buffer if any.
            if let Some(end) = resp_head.windows(4).position(|w| w == b"\r\n\r\n") {
                if end + 4 < resp_head.len() {
                    // already included in write_all of resp_head
                }
            }
            return raw_tunnel(client, up, log).await;
        }

        // Normal response: copy headers + body, then loop for keep-alive.
        let resp_head = read_http_headers(&mut up)
            .await?
            .ok_or_else(|| anyhow!("upstream closed before response"))?;
        let status = parse_status_from_headers(&resp_head);
        let resp_text = std::str::from_utf8(
            &resp_head[..resp_head
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .unwrap_or(resp_head.len())],
        )
        .unwrap_or("");
        let resp_cl = parse_content_length(resp_text);
        let resp_chunked = resp_text
            .to_ascii_lowercase()
            .contains("transfer-encoding: chunked");
        let resp_close = resp_text.to_ascii_lowercase().contains("connection: close");

        log.push(CaptureEvent {
            seq: 0,
            kind: "http_resp".into(),
            method: Some(method.clone()),
            path: Some(path.clone()),
            host: Some(target.display.clone()),
            status,
            body_len: resp_cl,
            note: None,
        });

        client.write_all(&resp_head).await?;
        // body_start: byte offset after \r\n\r\n in resp_head
        let resp_body_start = resp_head
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map(|p| p + 4)
            .unwrap_or(resp_head.len());
        let already = resp_head.len().saturating_sub(resp_body_start);
        let mut resp_capture = Vec::new();
        capture_extend(&mut resp_capture, &resp_head[resp_body_start..]);

        if resp_chunked {
            if already == 0 {
                copy_chunked_body_capture(&mut up, client, &mut resp_capture).await?;
            } else {
                copy_chunked_body_remaining_capture(
                    &mut up,
                    client,
                    &resp_head[resp_body_start..],
                    &mut resp_capture,
                )
                .await?;
            }
        } else if let Some(cl) = resp_cl {
            copy_fixed_body_capture(
                &mut up,
                client,
                cl.saturating_sub(already),
                &mut resp_capture,
            )
            .await?;
        } else if resp_close {
            // read until EOF
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let n = up.read(&mut buf).await?;
                if n == 0 {
                    break;
                }
                capture_extend(&mut resp_capture, &buf[..n]);
                client.write_all(&buf[..n]).await?;
            }
            append_http_body_capture(
                log,
                "http_resp_body",
                &method,
                &path,
                &resp_capture,
                resp_capture.len(),
                resp_chunked,
            );
            return Ok(());
        }
        append_http_body_capture(
            log,
            "http_resp_body",
            &method,
            &path,
            &resp_capture,
            resp_capture.len(),
            resp_chunked,
        );
        // else: no body (or unknown) — continue keep-alive loop
        client.flush().await.ok();
        if resp_close {
            return Ok(());
        }
    }
}

async fn raw_tunnel<U>(client: &mut TcpStream, mut up: U, log: &CaptureLog) -> Result<()>
where
    U: AsyncRead + AsyncWrite + Unpin,
{
    // Full-duplex until either side closes (required for Amp actor WS).
    // If the wrap is file-backed, parse WebSocket frames while forwarding so
    // Amp actor payloads are captured in ws.jsonl next to flows.jsonl.
    let Some(ws_path) = ws_capture_path(log) else {
        let _ = tokio::io::copy_bidirectional(client, &mut up).await;
        return Ok(());
    };

    let (client_r, client_w) = tokio::io::split(client);
    let (up_r, up_w) = tokio::io::split(up);
    let c2u = copy_ws_frames(client_r, up_w, ws_path.clone(), "client_to_upstream", true);
    let u2c = copy_ws_frames(up_r, client_w, ws_path, "upstream_to_client", false);
    tokio::select! {
        _ = c2u => {},
        _ = u2c => {},
    }
    Ok(())
}

fn ws_capture_path(log: &CaptureLog) -> Option<PathBuf> {
    if std::env::var_os("ALEX_WRAP_NO_WS_CAPTURE").is_some() {
        return None;
    }
    log.jsonl_path().map(|p| p.with_file_name("ws.jsonl"))
}

async fn copy_ws_frames<R, W>(
    mut reader: R,
    mut writer: W,
    path: PathBuf,
    direction: &'static str,
    client_masked: bool,
) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    loop {
        let mut h = [0u8; 2];
        if reader.read_exact(&mut h).await.is_err() {
            let _ = writer.shutdown().await;
            return Ok(());
        }
        writer.write_all(&h).await?;

        let fin = (h[0] & 0x80) != 0;
        let opcode = h[0] & 0x0f;
        let masked = (h[1] & 0x80) != 0;
        let mut len = (h[1] & 0x7f) as u64;

        let mut ext = Vec::new();
        if len == 126 {
            let mut b = [0u8; 2];
            reader.read_exact(&mut b).await?;
            writer.write_all(&b).await?;
            ext.extend_from_slice(&b);
            len = u16::from_be_bytes(b) as u64;
        } else if len == 127 {
            let mut b = [0u8; 8];
            reader.read_exact(&mut b).await?;
            writer.write_all(&b).await?;
            ext.extend_from_slice(&b);
            len = u64::from_be_bytes(b);
        }

        let mut mask = [0u8; 4];
        if masked {
            reader.read_exact(&mut mask).await?;
            writer.write_all(&mask).await?;
        } else if client_masked {
            // Client frames should be masked, but keep forwarding if not.
        }

        let cap_limit = ws_capture_limit();
        let mut captured = Vec::with_capacity((len.min(cap_limit as u64)) as usize);
        let mut remaining = len;
        let mut offset = 0u64;
        let mut buf = vec![0u8; 16 * 1024];
        while remaining > 0 {
            let take = remaining.min(buf.len() as u64) as usize;
            reader.read_exact(&mut buf[..take]).await?;
            writer.write_all(&buf[..take]).await?;

            if captured.len() < cap_limit {
                let room = cap_limit - captured.len();
                let n = room.min(take);
                if masked {
                    for i in 0..n {
                        captured.push(buf[i] ^ mask[((offset + i as u64) % 4) as usize]);
                    }
                } else {
                    captured.extend_from_slice(&buf[..n]);
                }
            }
            offset += take as u64;
            remaining -= take as u64;
        }
        writer.flush().await.ok();
        append_ws_capture(
            &path,
            direction,
            fin,
            opcode,
            masked,
            len,
            &captured,
            len as usize > cap_limit,
        );

        if opcode == 0x8 {
            let _ = writer.shutdown().await;
            return Ok(());
        }
    }
}

fn ws_capture_limit() -> usize {
    std::env::var("ALEX_WRAP_WS_CAPTURE_LIMIT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1024 * 1024)
}

fn append_ws_capture(
    path: &PathBuf,
    direction: &str,
    fin: bool,
    opcode: u8,
    masked: bool,
    payload_len: u64,
    captured: &[u8],
    truncated: bool,
) {
    let opcode_name = match opcode {
        0x0 => "continuation",
        0x1 => "text",
        0x2 => "binary",
        0x8 => "close",
        0x9 => "ping",
        0xA => "pong",
        _ => "unknown",
    };
    let text = std::str::from_utf8(captured).ok().map(redact_ws_text);
    let mut obj = serde_json::json!({
        "ts": chrono::Utc::now().to_rfc3339(),
        "direction": direction,
        "fin": fin,
        "opcode": opcode,
        "opcode_name": opcode_name,
        "masked": masked,
        "payload_len": payload_len,
        "captured_len": captured.len(),
        "truncated": truncated,
    });
    if let Some(t) = text {
        obj["text"] = serde_json::Value::String(t);
    } else if !captured.is_empty() {
        obj["base64"] =
            serde_json::Value::String(base64::engine::general_purpose::STANDARD.encode(captured));
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        use std::io::Write;
        let _ = writeln!(f, "{}", obj);
    }
}

fn redact_ws_text(s: &str) -> String {
    let mut out = s.to_string();
    for key in [
        "authorization",
        "apiKey",
        "api_key",
        "token",
        "rvt-token",
        "wsToken",
    ] {
        out = redact_jsonish_value(&out, key);
    }
    out
}

fn redact_jsonish_value(s: &str, key: &str) -> String {
    // Lightweight best-effort redaction for JSON-ish websocket text payloads.
    let needle = format!("\"{key}\"");
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(pos) = rest.find(&needle) {
        out.push_str(&rest[..pos + needle.len()]);
        rest = &rest[pos + needle.len()..];
        let Some(colon) = rest.find(':') else {
            break;
        };
        out.push_str(&rest[..colon + 1]);
        rest = &rest[colon + 1..];
        let ws = rest.len() - rest.trim_start().len();
        out.push_str(&rest[..ws]);
        rest = &rest[ws..];
        if let Some(after) = rest.strip_prefix('"') {
            if let Some(end) = after.find('"') {
                out.push_str("\"<redacted>\"");
                rest = &after[end + 1..];
                continue;
            }
        }
        out.push_str("\"<redacted>\"");
        if let Some(next) = rest.find([',', '}']) {
            rest = &rest[next..];
        } else {
            rest = "";
        }
    }
    out.push_str(rest);
    out
}

/// Dial upstream, preferring IPv4, with a hard timeout so local firewall denials
/// (silent SYN drop) fail fast instead of stranding the client.
fn socket_ip_is_public(addr: &std::net::SocketAddr) -> bool {
    match addr.ip() {
        std::net::IpAddr::V4(ip) => {
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_documentation()
                || ip.octets()[0] == 0
                || ip.octets()[0] >= 224
                || (ip.octets()[0] == 100 && (ip.octets()[1] & 0b1100_0000) == 0b0100_0000))
        }
        std::net::IpAddr::V6(ip) => {
            !(ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_unique_local()
                || ip.is_unicast_link_local())
        }
    }
}

async fn dial_upstream(target: &UpstreamTarget) -> Result<TcpStream> {
    let host_port = format!("{}:{}", target.host, target.port);
    let addrs: Vec<std::net::SocketAddr> = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::net::lookup_host(&host_port),
    )
    .await
    .with_context(|| format!("timeout resolving {host_port}"))?
    .with_context(|| format!("resolve {host_port}"))?
    .collect();
    if addrs.is_empty() {
        bail!("no addresses for {host_port}");
    }
    // Prefer public IPv4. Some vendor DNS answers include 100.64/10 CGNAT
    // addresses that can accept TCP on developer machines but stall TLS unless
    // the vendor private network is active.
    let mut ordered = addrs;
    ordered.sort_by_key(|a| {
        let public = socket_ip_is_public(a);
        match (public, a.is_ipv4()) {
            (true, true) => 0,
            (true, false) => 1,
            (false, true) => 2,
            (false, false) => 3,
        }
    });

    let mut last_err = None;
    for addr in ordered {
        match tokio::time::timeout(std::time::Duration::from_secs(10), TcpStream::connect(addr))
            .await
        {
            Ok(Ok(s)) => return Ok(s),
            Ok(Err(e)) => last_err = Some(anyhow!("connect {addr}: {e}")),
            Err(_) => {
                last_err = Some(anyhow!(
                "timeout connect {addr} (check local firewall / Little Snitch allow for alex → {})",
                target.host
            ))
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("connect {host_port} failed")))
}

async fn read_http_headers<R: AsyncRead + Unpin>(r: &mut R) -> Result<Option<Vec<u8>>> {
    let mut buf = Vec::with_capacity(4096);
    let mut tmp = [0u8; 2048];
    loop {
        let n = r.read(&mut tmp).await?;
        if n == 0 {
            return if buf.is_empty() {
                Ok(None)
            } else {
                // incomplete
                Ok(Some(buf))
            };
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            return Ok(Some(buf));
        }
        if buf.len() > 1024 * 1024 {
            bail!("headers too large");
        }
    }
}

const HTTP_BODY_CAPTURE_LIMIT: usize = 1024 * 1024;

fn capture_extend(out: &mut Vec<u8>, bytes: &[u8]) {
    if out.len() >= HTTP_BODY_CAPTURE_LIMIT || bytes.is_empty() {
        return;
    }
    let room = HTTP_BODY_CAPTURE_LIMIT - out.len();
    out.extend_from_slice(&bytes[..bytes.len().min(room)]);
}

fn append_http_body_capture(
    log: &CaptureLog,
    kind: &str,
    method: &str,
    path: &str,
    captured: &[u8],
    observed_len: usize,
    chunked: bool,
) {
    if captured.is_empty() {
        return;
    }
    if !log.should_record_path(path) {
        return;
    }
    let Some(flows) = log.jsonl_path() else {
        return;
    };
    let dest = flows.with_file_name("http-bodies.jsonl");
    let max_preview = log.policy().max_body_preview_bytes;
    let preview_len = captured.len().min(max_preview);
    let preview = &captured[..preview_len];
    let text = std::str::from_utf8(preview).ok().map(redact_ws_text);
    let mut obj = serde_json::json!({
        "ts": chrono::Utc::now().to_rfc3339(),
        "kind": kind,
        "method": method,
        "path": log.redact_path(path),
        "observed_len": observed_len,
        "captured_len": preview_len,
        "truncated": captured.len() > preview_len || observed_len > preview_len,
        "chunked": chunked,
    });
    if let Some(t) = text {
        obj["text"] = serde_json::Value::String(t);
    } else {
        // Binary payloads are useful as metadata but are not persisted: they
        // can contain opaque credentials and cannot be reliably redacted.
        obj["binary"] = serde_json::Value::Bool(true);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dest)
    {
        use std::io::Write;
        let _ = writeln!(f, "{}", obj);
    }
}

async fn copy_fixed_body_capture<R, W>(
    r: &mut R,
    w: &mut W,
    mut remaining: usize,
    capture: &mut Vec<u8>,
) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buf = vec![0u8; 64 * 1024];
    while remaining > 0 {
        let to_read = remaining.min(buf.len());
        let n = r.read(&mut buf[..to_read]).await?;
        if n == 0 {
            bail!("unexpected EOF reading body ({remaining} left)");
        }
        capture_extend(capture, &buf[..n]);
        w.write_all(&buf[..n]).await?;
        remaining -= n;
    }
    Ok(())
}

/// Copy HTTP/1.1 chunked body including terminating 0-chunk.
async fn copy_chunked_body_capture<R, W>(r: &mut R, w: &mut W, capture: &mut Vec<u8>) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut line = Vec::new();
    loop {
        line.clear();
        read_until_crlf(r, &mut line).await?;
        capture_extend(capture, &line);
        w.write_all(&line).await?;
        let hex = std::str::from_utf8(&line)
            .unwrap_or("")
            .trim()
            .split(';')
            .next()
            .unwrap_or("0");
        let size = usize::from_str_radix(hex.trim(), 16).unwrap_or(0);
        if size > 0 {
            copy_fixed_body_capture(r, w, size, capture).await?;
        }
        // trailing CRLF after chunk data
        let mut crlf = [0u8; 2];
        r.read_exact(&mut crlf).await?;
        capture_extend(capture, &crlf);
        w.write_all(&crlf).await?;
        if size == 0 {
            // optional trailers until blank line
            loop {
                line.clear();
                read_until_crlf(r, &mut line).await?;
                capture_extend(capture, &line);
                w.write_all(&line).await?;
                if line == b"\r\n" || line == b"\n" {
                    break;
                }
            }
            break;
        }
    }
    Ok(())
}

/// Continue parsing/copying a chunked body when some body bytes were already
/// read together with the request headers and already forwarded by the header
/// rewrite path.
async fn copy_chunked_body_remaining_capture<R, W>(
    r: &mut R,
    w: &mut W,
    initial: &[u8],
    capture: &mut Vec<u8>,
) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buf = initial.to_vec();
    let mut pos = 0usize;
    loop {
        let line_start = pos;
        loop {
            if let Some(rel) = buf[pos..].windows(2).position(|x| x == b"\r\n") {
                pos += rel + 2;
                break;
            }
            read_more_forward_capture(r, w, &mut buf, capture).await?;
        }
        let line = &buf[line_start..pos];
        let hex = std::str::from_utf8(line)
            .unwrap_or("")
            .trim()
            .split(';')
            .next()
            .unwrap_or("0");
        let size = usize::from_str_radix(hex.trim(), 16).unwrap_or(0);
        while buf.len().saturating_sub(pos) < size + 2 {
            read_more_forward_capture(r, w, &mut buf, capture).await?;
        }
        pos += size + 2;
        if size == 0 {
            if pos >= 4 && &buf[pos - 4..pos] == b"\r\n\r\n" {
                return Ok(());
            }
            loop {
                let trailer_start = pos;
                loop {
                    if let Some(rel) = buf[pos..].windows(2).position(|x| x == b"\r\n") {
                        pos += rel + 2;
                        break;
                    }
                    read_more_forward_capture(r, w, &mut buf, capture).await?;
                }
                if &buf[trailer_start..pos] == b"\r\n" {
                    return Ok(());
                }
            }
        }
        if pos > 1024 * 1024 {
            buf.drain(..pos);
            pos = 0;
        }
    }
}

async fn read_more_forward_capture<R, W>(
    r: &mut R,
    w: &mut W,
    buf: &mut Vec<u8>,
    capture: &mut Vec<u8>,
) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut tmp = [0u8; 8192];
    let n = r.read(&mut tmp).await?;
    if n == 0 {
        bail!("unexpected EOF reading chunked body");
    }
    capture_extend(capture, &tmp[..n]);
    w.write_all(&tmp[..n]).await?;
    buf.extend_from_slice(&tmp[..n]);
    Ok(())
}

async fn read_until_crlf<R: AsyncRead + Unpin>(r: &mut R, out: &mut Vec<u8>) -> Result<()> {
    let mut b = [0u8; 1];
    loop {
        r.read_exact(&mut b).await?;
        out.push(b[0]);
        if out.len() >= 2 && out[out.len() - 2..] == *b"\r\n" {
            return Ok(());
        }
        if out.len() > 64 * 1024 {
            bail!("line too long");
        }
    }
}

fn parse_content_length(headers: &str) -> Option<usize> {
    for line in headers.lines() {
        if let Some((n, v)) = line.split_once(':') {
            if n.eq_ignore_ascii_case("content-length") {
                return v.trim().parse().ok();
            }
        }
    }
    None
}

fn parse_status_from_headers(head: &[u8]) -> Option<u16> {
    let text = std::str::from_utf8(head).ok()?;
    let line = text.lines().next()?;
    line.split_whitespace().nth(1)?.parse().ok()
}

fn rewrite_request_headers(
    head: &[u8],
    host: &str,
    port: u16,
    tls: bool,
    inject: Option<&WrapReverseInject>,
) -> Result<Vec<u8>> {
    let end = head
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| anyhow!("incomplete headers"))?;
    let header_block =
        std::str::from_utf8(&head[..end]).context("request headers are not utf-8")?;
    let body = &head[end + 4..];

    let lines: Vec<&str> = header_block.split("\r\n").collect();
    if lines.is_empty() {
        bail!("empty request");
    }
    let req_line = inject_query_params(lines[0], inject);
    let host_value = if (!tls && port == 80) || (tls && port == 443) {
        host.to_string()
    } else {
        format!("{host}:{port}")
    };
    let scheme = if tls { "https" } else { "http" };
    let origin_value = format!("{scheme}://{host_value}");

    let mut out_lines: Vec<String> = Vec::with_capacity(lines.len() + 2);
    out_lines.push(req_line);
    let mut saw_host = false;
    let mut saw_origin = false;
    for line in lines.iter().skip(1) {
        if line.is_empty() {
            continue;
        }
        if let Some((name, _)) = line.split_once(':') {
            let n = name.trim();
            if n.eq_ignore_ascii_case("host") {
                out_lines.push(format!("Host: {host_value}"));
                saw_host = true;
                continue;
            }
            if n.eq_ignore_ascii_case("origin") {
                out_lines.push(format!("Origin: {origin_value}"));
                saw_origin = true;
                continue;
            }
            if n.eq_ignore_ascii_case("referer") {
                out_lines.push(format!("Referer: {origin_value}/"));
                continue;
            }
            // Prefer uncompressed responses so keep-alive body framing is simple.
            if n.eq_ignore_ascii_case("accept-encoding") {
                out_lines.push("Accept-Encoding: identity".into());
                continue;
            }
        }
        out_lines.push((*line).to_string());
    }
    if !saw_host {
        out_lines.push(format!("Host: {host_value}"));
    }
    if !saw_origin {
        out_lines.push(format!("Origin: {origin_value}"));
    }

    let mut out = out_lines.join("\r\n").into_bytes();
    out.extend_from_slice(b"\r\n\r\n");
    out.extend_from_slice(body);
    Ok(out)
}

/// Inject catalog query params into `METHOD path?query HTTP/x.y` when path matches.
fn inject_query_params(req_line: &str, inject: Option<&WrapReverseInject>) -> String {
    let Some(inj) = inject else {
        return req_line.to_string();
    };
    if inj.query_params.is_empty() {
        return req_line.to_string();
    }
    let mut parts = req_line.splitn(3, ' ');
    let method = match parts.next() {
        Some(m) => m,
        None => return req_line.to_string(),
    };
    let target = match parts.next() {
        Some(t) => t,
        None => return req_line.to_string(),
    };
    let version = parts.next().unwrap_or("HTTP/1.1");

    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (target, None),
    };
    if !inj.path_prefixes.is_empty()
        && !inj
            .path_prefixes
            .iter()
            .any(|p| path.starts_with(p.as_str()))
    {
        return req_line.to_string();
    }

    let mut pairs: Vec<(String, String)> = Vec::new();
    if let Some(q) = query {
        for part in q.split('&') {
            if part.is_empty() {
                continue;
            }
            if let Some((k, v)) = part.split_once('=') {
                pairs.push((k.to_string(), v.to_string()));
            } else {
                pairs.push((part.to_string(), String::new()));
            }
        }
    }

    for (k, v) in &inj.query_params {
        let present = pairs.iter().any(|(pk, _)| pk == k);
        if present && inj.only_if_missing {
            continue;
        }
        if present {
            if let Some((_, pv)) = pairs.iter_mut().find(|(pk, _)| pk == k) {
                *pv = v.clone();
            }
        } else {
            pairs.push((k.clone(), v.clone()));
        }
    }

    let new_query = pairs
        .into_iter()
        .map(|(k, v)| if v.is_empty() { k } else { format!("{k}={v}") })
        .collect::<Vec<_>>()
        .join("&");
    let new_target = if new_query.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{new_query}")
    };
    format!("{method} {new_target} {version}")
}

fn split_headers(buf: &[u8]) -> Option<(&str, usize)> {
    let end = buf.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&buf[..end]).ok()?;
    let line = head.lines().next()?;
    Some((line, end + 4))
}

fn dump_headers(method: &str, path: &str, head: &[u8]) {
    if let Ok(s) = std::str::from_utf8(head) {
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/alex-wrap-headers.dump")
            .and_then(|mut f| {
                use std::io::Write;
                writeln!(f, "===== {method} {path} =====")?;
                for line in s.lines().take(40) {
                    let lower = line.to_ascii_lowercase();
                    if lower.starts_with("authorization:")
                        || lower.starts_with("cookie:")
                        || lower.starts_with("x-api-key:")
                    {
                        writeln!(f, "{}: <redacted>", line.split(':').next().unwrap_or("?"))?;
                    } else {
                        writeln!(f, "{line}")?;
                    }
                }
                writeln!(f)
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn parse_upstream_https() {
        let t = UpstreamTarget::parse("https://ampcode.com").unwrap();
        assert!(t.tls);
        assert_eq!(t.host, "ampcode.com");
        assert_eq!(t.port, 443);
    }

    #[test]
    fn rewrite_host() {
        let raw = b"GET /api/x HTTP/1.1\r\nHost: 127.0.0.1:9\r\nConnection: close\r\n\r\n";
        let out = rewrite_request_headers(raw, "ampcode.com", 443, true, None).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("Host: ampcode.com\r\n"));
        assert!(s.contains("Origin: https://ampcode.com\r\n"));
        assert!(!s.contains("127.0.0.1:9"));
    }

    #[test]
    fn inject_rvt_token_on_actors_path() {
        use crate::catalog::WrapReverseInject;
        use std::collections::BTreeMap;
        let mut query_params = BTreeMap::new();
        query_params.insert("rvt-token".into(), "pk_test".into());
        let inj = WrapReverseInject {
            notes: None,
            query_params,
            path_prefixes: vec!["/actors/".into()],
            only_if_missing: true,
        };
        let raw = b"GET /actors/gateway/threadActor/websocket/?rvt-namespace=default&rvt-key=T-1&rvt-skip-ready-wait=true HTTP/1.1\r\nHost: 127.0.0.1:9\r\n\r\n";
        let out = rewrite_request_headers(raw, "ampcode.com", 443, true, Some(&inj)).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("rvt-token=pk_test"), "s={s}");
        // non-actors path unchanged
        let raw2 = b"GET /api/x?foo=1 HTTP/1.1\r\nHost: 127.0.0.1:9\r\n\r\n";
        let out2 = rewrite_request_headers(raw2, "ampcode.com", 443, true, Some(&inj)).unwrap();
        let s2 = String::from_utf8(out2).unwrap();
        assert!(!s2.contains("rvt-token"), "s2={s2}");
    }

    #[test]
    fn inject_does_not_overwrite_existing_token() {
        use crate::catalog::WrapReverseInject;
        use std::collections::BTreeMap;
        let mut query_params = BTreeMap::new();
        query_params.insert("rvt-token".into(), "pk_new".into());
        let inj = WrapReverseInject {
            notes: None,
            query_params,
            path_prefixes: vec!["/actors/".into()],
            only_if_missing: true,
        };
        let raw = b"GET /actors/x?rvt-token=pk_old HTTP/1.1\r\nHost: 127.0.0.1:9\r\n\r\n";
        let out = rewrite_request_headers(raw, "ampcode.com", 443, true, Some(&inj)).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("rvt-token=pk_old"), "s={s}");
        assert!(!s.contains("pk_new"));
    }

    #[tokio::test]
    async fn reverse_wrap_captures_path_and_forwards() {
        let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let up_addr = upstream.local_addr().unwrap();
        let up_task = tokio::spawn(async move {
            let (mut sock, _) = upstream.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let n = sock.read(&mut buf).await.unwrap();
            let req = String::from_utf8_lossy(&buf[..n]);
            assert!(req.contains("GET /api/internal?getUserInfo"));
            let body = br#"{"ok":true}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            sock.write_all(resp.as_bytes()).await.unwrap();
            sock.write_all(body).await.unwrap();
        });

        let log = CaptureLog::new();
        let wrap =
            ReverseWrap::start_http_to_http("127.0.0.1:0".parse().unwrap(), up_addr, log.clone())
                .await
                .unwrap();

        let mut client = TcpStream::connect(wrap.listen_addr).await.unwrap();
        client
            .write_all(
                b"GET /api/internal?getUserInfo HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        let mut resp = Vec::new();
        client.read_to_end(&mut resp).await.unwrap();
        let resp_s = String::from_utf8_lossy(&resp);
        assert!(resp_s.contains("200 OK"), "resp={resp_s}");

        let paths = log.paths();
        assert!(
            paths
                .iter()
                .any(|p| p.contains("/api/internal?getUserInfo")),
            "paths={paths:?}"
        );

        wrap.shutdown().await;
        up_task.await.unwrap();
    }

    #[tokio::test]
    async fn reverse_wrap_keep_alive_two_requests() {
        let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let up_addr = upstream.local_addr().unwrap();
        let up_task = tokio::spawn(async move {
            let (mut sock, _) = upstream.accept().await.unwrap();
            for _ in 0..2 {
                let mut buf = vec![0u8; 8192];
                // read until headers complete
                let mut n = 0;
                loop {
                    let k = sock.read(&mut buf[n..]).await.unwrap();
                    n += k;
                    if buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let body = b"ok";
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n",
                    body.len()
                );
                sock.write_all(resp.as_bytes()).await.unwrap();
                sock.write_all(body).await.unwrap();
            }
        });

        let log = CaptureLog::new();
        let wrap =
            ReverseWrap::start_http_to_http("127.0.0.1:0".parse().unwrap(), up_addr, log.clone())
                .await
                .unwrap();

        let mut client = TcpStream::connect(wrap.listen_addr).await.unwrap();
        for path in ["/api/a", "/api/b"] {
            let req =
                format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: keep-alive\r\n\r\n");
            client.write_all(req.as_bytes()).await.unwrap();
            // read response
            let mut buf = vec![0u8; 4096];
            let mut n = 0;
            loop {
                let k = client.read(&mut buf[n..]).await.unwrap();
                n += k;
                if buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
                    // if content-length, ensure body
                    let text = String::from_utf8_lossy(&buf[..n]);
                    if text.contains("Content-Length: 2") && n >= text.find("\r\n\r\n").unwrap() + 6
                    {
                        break;
                    }
                    if text.contains("\r\nok") {
                        break;
                    }
                }
                if k == 0 {
                    break;
                }
            }
            assert!(String::from_utf8_lossy(&buf[..n]).contains("200 OK"));
        }

        let paths = log.paths();
        assert!(paths.iter().any(|p| p.contains("/api/a")), "{paths:?}");
        assert!(paths.iter().any(|p| p.contains("/api/b")), "{paths:?}");

        wrap.shutdown().await;
        up_task.await.unwrap();
    }
}
