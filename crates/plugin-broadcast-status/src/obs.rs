//! obs-websocket v5: `Hello` → `Identify` (with the password's challenge response), a snapshot
//! of the stream, the recording and the scene, then events only. It reads; it never asks OBS
//! to start, stop or switch anything.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// Event subscriptions: Scenes (1 << 2) and Outputs (1 << 6).
const EVENTS: u64 = (1 << 2) | (1 << 6);
/// What's asked once identified; events keep it current after that.
const SNAPSHOT: [&str; 3] = ["GetStreamStatus", "GetRecordStatus", "GetCurrentProgramScene"];
/// OBS's close code for a wrong password.
const AUTH_FAILED: u16 = 4009;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub streaming: bool,
    pub recording: bool,
    pub paused: bool,
    pub scene: Option<String>,
}

/// The challenge response: base64(sha256(base64(sha256(password + salt)) + challenge)).
pub fn auth(password: &str, salt: &str, challenge: &str) -> String {
    let secret = STANDARD.encode(Sha256::digest(format!("{password}{salt}")));
    STANDARD.encode(Sha256::digest(format!("{secret}{challenge}")))
}

/// One connection's conversation.
pub struct Session {
    password: Option<String>,
    pub status: Status,
    pub identified: bool,
    /// Snapshot answers still to come.
    waiting: usize,
}

impl Session {
    pub fn new(password: Option<String>) -> Session {
        Session { password, status: Status::default(), identified: false, waiting: 0 }
    }

    /// Identified, with the snapshot in: the status is worth showing.
    pub fn ready(&self) -> bool {
        self.identified && self.waiting == 0
    }

    /// Handles a message from OBS: what to send back, or why the connection can't go on.
    pub fn on_message(&mut self, text: &str) -> Result<Vec<Value>, String> {
        let msg: Value = serde_json::from_str(text).map_err(|e| format!("OBS sent something odd: {e}"))?;
        let d = &msg["d"];
        Ok(match msg["op"].as_u64() {
            // Hello
            Some(0) => {
                let mut identify = json!({"rpcVersion": 1, "eventSubscriptions": EVENTS});
                if let Some(a) = d.get("authentication") {
                    let password = self.password.as_deref().ok_or("OBS asks for a password: set the OBS WebSocket password in the Plugins tab")?;
                    let (salt, challenge) = (a["salt"].as_str().unwrap_or_default(), a["challenge"].as_str().unwrap_or_default());
                    identify["authentication"] = json!(auth(password, salt, challenge));
                }
                vec![json!({"op": 1, "d": identify})]
            }
            // Identified: take a snapshot.
            Some(2) => {
                self.identified = true;
                self.waiting = SNAPSHOT.len();
                SNAPSHOT
                    .map(|r| json!({"op": 6, "d": {"requestType": r, "requestId": r}}))
                    .to_vec()
            }
            // Event
            Some(5) => {
                let e = &d["eventData"];
                let s = &mut self.status;
                match d["eventType"].as_str() {
                    Some("StreamStateChanged") => s.streaming = e["outputActive"] == true,
                    Some("RecordStateChanged") => {
                        s.recording = e["outputActive"] == true;
                        s.paused = s.recording && e["outputState"] == "OBS_WEBSOCKET_OUTPUT_PAUSED";
                    }
                    Some("CurrentProgramSceneChanged") => s.scene = e["sceneName"].as_str().map(String::from),
                    _ => {}
                }
                vec![]
            }
            // A snapshot's answer.
            Some(7) => {
                let r = &d["responseData"];
                self.waiting = self.waiting.saturating_sub(1);
                let s = &mut self.status;
                if d["requestStatus"]["result"] == true {
                    match d["requestType"].as_str() {
                        Some("GetStreamStatus") => s.streaming = r["outputActive"] == true,
                        Some("GetRecordStatus") => {
                            s.recording = r["outputActive"] == true;
                            s.paused = s.recording && r["outputPaused"] == true;
                        }
                        Some("GetCurrentProgramScene") => {
                            s.scene = r["currentProgramSceneName"].as_str().or(r["sceneName"].as_str()).map(String::from)
                        }
                        _ => {}
                    }
                }
                vec![]
            }
            _ => vec![],
        })
    }
}

/// Why OBS closed the connection, in words.
pub fn closed(code: Option<u16>) -> String {
    match code {
        Some(AUTH_FAILED) => "OBS refused the password: check the OBS WebSocket password in the Plugins tab".into(),
        Some(c) => format!("OBS closed the connection ({c})"),
        None => "OBS closed the connection".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_challenge_response_matches_obs() {
        // Worked out separately with Python's hashlib.
        assert_eq!(
            auth("supersecretpassword", "lM1GncleQOaCu9lT1yeUZhFYnqhsLLP1G5lAGo3ixaI=", "+IxH4CnCiqpX1rM9scsNynZzbOe4KhDeYcTNS3PDaeY="),
            "1Ct943GAT+6YQUUX47Ia/ncufilbe6+oD6lY+5kaCu4="
        );
    }

    #[test]
    fn hello_identify_snapshot_and_events() {
        let mut s = Session::new(Some("pw".into()));
        let hello = json!({"op": 0, "d": {"rpcVersion": 1, "authentication": {"challenge": "c", "salt": "s"}}});
        let out = s.on_message(&hello.to_string()).unwrap();
        assert_eq!(out[0]["op"], 1);
        assert_eq!(out[0]["d"]["authentication"], json!(auth("pw", "s", "c")));
        assert_eq!(out[0]["d"]["eventSubscriptions"], json!(68));
        let asks = s.on_message(r#"{"op":2,"d":{"negotiatedRpcVersion":1}}"#).unwrap();
        assert!(s.identified && !s.ready());
        assert_eq!(asks.len(), 3);
        assert!(asks.iter().all(|a| a["op"] == 6), "only Get requests: {asks:?}");

        let answer = |t: &str, data: Value| json!({"op": 7, "d": {"requestType": t, "requestId": t, "requestStatus": {"result": true, "code": 100}, "responseData": data}}).to_string();
        s.on_message(&answer("GetStreamStatus", json!({"outputActive": false}))).unwrap();
        s.on_message(&answer("GetRecordStatus", json!({"outputActive": true, "outputPaused": true}))).unwrap();
        assert!(!s.ready());
        s.on_message(&answer("GetCurrentProgramScene", json!({"currentProgramSceneName": "Gameplay"}))).unwrap();
        assert!(s.ready());
        assert_eq!(s.status, Status { streaming: false, recording: true, paused: true, scene: Some("Gameplay".into()) });

        let event = |t: &str, data: Value| json!({"op": 5, "d": {"eventType": t, "eventIntent": 64, "eventData": data}}).to_string();
        s.on_message(&event("RecordStateChanged", json!({"outputActive": true, "outputState": "OBS_WEBSOCKET_OUTPUT_RESUMED"}))).unwrap();
        s.on_message(&event("StreamStateChanged", json!({"outputActive": true, "outputState": "OBS_WEBSOCKET_OUTPUT_STARTED"}))).unwrap();
        s.on_message(&event("CurrentProgramSceneChanged", json!({"sceneName": "BRB"}))).unwrap();
        assert_eq!(s.status, Status { streaming: true, recording: true, paused: false, scene: Some("BRB".into()) });
        s.on_message(&event("RecordStateChanged", json!({"outputActive": false, "outputState": "OBS_WEBSOCKET_OUTPUT_STOPPED"}))).unwrap();
        assert!(!s.status.recording);
    }

    #[test]
    fn no_password_and_a_wrong_one_say_so() {
        let mut s = Session::new(None);
        let hello = json!({"op": 0, "d": {"authentication": {"challenge": "c", "salt": "s"}}});
        assert!(s.on_message(&hello.to_string()).unwrap_err().contains("password"));
        // No authentication asked: no password needed.
        let out = s.on_message(r#"{"op":0,"d":{"rpcVersion":1}}"#).unwrap();
        assert!(out[0]["d"].get("authentication").is_none());
        assert!(closed(Some(4009)).contains("refused the password"));
    }
}
