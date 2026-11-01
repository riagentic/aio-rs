//! `[feedback] url` (aio's `feedback.url`): each problem report, once it is on
//! disk, POSTed there as JSON (`aio::report::sink`). A collector that takes
//! longer than `[feedback] timeout` has not got it; the report stays saved.
use std::time::Duration;

/// POST `report` to `url`; `Err` unless it answers 2xx within `timeout`.
pub(crate) fn post(url: &str, timeout: Duration, report: &aio::serde_json::Value) -> Result<(), String> {
    let agent = aio::capabilities::agent(
        "feedback",
        ureq::Agent::config_builder().timeout_global(Some(timeout)).http_status_as_error(false).build(),
    );
    let res = agent
        .post(url)
        .header("content-type", "application/json")
        .send(report.to_string())
        .map_err(|e| format!("{url}: {e}"))?;
    match res.status().as_u16() {
        200..=299 => Ok(()),
        s => Err(format!("{url} answered {s}")),
    }
}

/// Send reports on to `[feedback] url`, when there is one.
pub(crate) fn wire(cfg: &aio::config::Config) {
    if let Some(url) = cfg.feedback.url.clone() {
        let timeout = cfg.feedback.timeout;
        aio::report::sink(timeout, move |r| post(&url, timeout, r));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    /// A collector on loopback answering `status` (none: never answering),
    /// and what it was sent.
    fn collector(status: Option<u16>) -> (String, std::thread::JoinHandle<String>) {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/in", l.local_addr().unwrap());
        let got = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            // The head, then as much body as it says.
            let (mut req, mut buf) = (vec![], [0; 4096]);
            let whole = |r: &[u8]| {
                let t = String::from_utf8_lossy(r);
                t.split_once("\r\n\r\n").is_some_and(|(head, body)| {
                    let len = head.lines().find_map(|l| l.strip_prefix("content-length: ")?.parse().ok());
                    body.len() >= len.unwrap_or(0)
                })
            };
            while !whole(&req) {
                let n = s.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                req.extend(&buf[..n]);
            }
            match status {
                Some(c) => _ = write!(s, "HTTP/1.1 {c} X\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"),
                None => std::thread::sleep(Duration::from_secs(2)),
            }
            String::from_utf8_lossy(&req).into_owned()
        });
        (url, got)
    }

    #[test]
    fn a_report_is_posted_as_json() {
        let (url, got) = collector(Some(204));
        post(&url, Duration::from_secs(5), &aio::serde_json::json!({ "id": "r1" })).unwrap();
        let req = got.join().unwrap();
        assert!(req.starts_with("POST /in ") && req.contains("content-type: application/json"), "{req}");
        assert!(req.ends_with(r#"{"id":"r1"}"#), "{req}");
    }

    #[test]
    fn a_collector_that_refuses_or_never_answers_has_not_got_it() {
        let (url, _got) = collector(Some(500));
        assert!(post(&url, Duration::from_secs(5), &aio::serde_json::json!({})).unwrap_err().contains("answered 500"));
        let (url, _got) = collector(None);
        let t = std::time::Instant::now();
        assert!(post(&url, Duration::from_millis(300), &aio::serde_json::json!({})).is_err());
        assert!(t.elapsed() < Duration::from_secs(20), "given up at the timeout: {:?}", t.elapsed());
    }
}
