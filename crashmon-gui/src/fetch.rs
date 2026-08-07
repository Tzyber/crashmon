//! Auto-Nachschlagen unbekannter Fehler (Auto-Learning).
//!
//! Quelle: DuckDuckGo Instant-Answer-API (kostenlos, kein API-Key,
//! maschinenlesbares JSON). Bei unbekannten Xid-Codes (nicht in der
//! lokalen Wissensbasis) fragt die GUI automatisch nach und haengt das
//! Ergebnis mit Quell-URL an `knowledge.md` an.
//!
//! Parse-Logik ist pure + getestet; nur der HTTP-Abruf nutzt ureq (sync,
//! 10-s-Timeout — laeuft im GUI-Thread, ist selten und klein).

use std::io::Read;

/// Antwort der DDG-Instant-Answer-API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub text: String,
    pub url: String,
}

/// DDG-API-URL fuer eine Suchanfrage (pure, testbar).
pub fn search_url(query: &str) -> String {
    let q = urlencode(query);
    format!("https://api.duckduckgo.com/?q={q}&format=json&no_html=1&no_redirect=1")
}

/// Minimaler URL-Encoder (fuer Suchanfragen; Leerzeichen -> %20).
fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b' ' => out.push_str("%20"),
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Parst die DDG-Antwort: Abstract + AbstractURL, sonst der erste
/// RelatedTopic mit Text + URL. `None` wenn nichts Brauchbares.
pub fn parse_instant_answer(json: &str) -> Option<Answer> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    if let Some(text) = v.get("Abstract").and_then(|t| t.as_str())
        && !text.is_empty()
    {
        let url = v
            .get("AbstractURL")
            .and_then(|u| u.as_str())
            .unwrap_or("")
            .to_string();
        return Some(Answer {
            text: text.to_string(),
            url,
        });
    }
    // Fallback: erster RelatedTopic mit Inhalt (z. B. NVIDIA-Forum-Treffer)
    for topic in v
        .get("RelatedTopics")
        .and_then(|t| t.as_array())
        .into_iter()
        .flatten()
    {
        if let (Some(text), Some(url)) = (
            topic.get("Text").and_then(|t| t.as_str()),
            topic.get("FirstURL").and_then(|u| u.as_str()),
        ) {
            return Some(Answer {
                text: text.to_string(),
                url: url.to_string(),
            });
        }
    }
    None
}

/// HTTP-Abruf mit 10-s-Timeout. Fehler als String (GUI-Statuszeile).
pub fn fetch_answer(query: &str) -> Result<Option<Answer>, String> {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(10)))
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let response = agent
        .get(&search_url(query))
        .header("User-Agent", "crashmon-gui/0.1 (auto-lookup)")
        .call()
        .map_err(|e| format!("Abruf fehlgeschlagen: {e}"))?;
    let mut body = String::new();
    response
        .into_body()
        .into_reader()
        .read_to_string(&mut body)
        .map_err(|e| format!("Antwort unlesbar: {e}"))?;
    Ok(parse_instant_answer(&body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_url_encodes_query() {
        assert_eq!(
            search_url("NVRM Xid 31"),
            "https://api.duckduckgo.com/?q=NVRM%20Xid%2031&format=json&no_html=1&no_redirect=1"
        );
    }

    #[test]
    fn parse_abstract() {
        let json = r#"{"Abstract": "Xid 31 ist ein illegaler Speicherzugriff.", "AbstractURL": "https://example.com/xid31"}"#;
        assert_eq!(
            parse_instant_answer(json),
            Some(Answer {
                text: "Xid 31 ist ein illegaler Speicherzugriff.".into(),
                url: "https://example.com/xid31".into(),
            })
        );
    }

    #[test]
    fn parse_falls_back_to_related_topic() {
        let json = r#"{"Abstract": "", "RelatedTopics": [
            {"Text": "NVRM Xid 31 — illegal memory access", "FirstURL": "https://forum.example/xid31"}
        ]}"#;
        assert_eq!(
            parse_instant_answer(json),
            Some(Answer {
                text: "NVRM Xid 31 — illegal memory access".into(),
                url: "https://forum.example/xid31".into(),
            })
        );
    }

    #[test]
    fn parse_empty_is_none() {
        assert_eq!(parse_instant_answer(r#"{"Abstract": ""}"#), None);
        assert_eq!(parse_instant_answer("kein json"), None);
    }
}
