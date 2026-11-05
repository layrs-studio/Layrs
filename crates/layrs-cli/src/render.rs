use serde_json::{Value, json};

#[derive(Debug, Clone)]
pub struct Rendered {
    pub human: String,
    pub data: Value,
}

pub fn ok_json(data: Value) -> String {
    json!({ "ok": true, "data": data }).to_string()
}

pub fn error_json(message: &str) -> String {
    json!({ "ok": false, "error": { "message": message } }).to_string()
}
