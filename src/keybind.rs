//! `keybindings.json` and the `server.upsertKeybinding` / `server.removeKeybinding` RPCs.
//! The file holds only the user's rules; the resolved list sent to clients is T3's defaults with
//! every command the user bound replaced (T3 `mergeWithDefaultKeybindings`). Parsing mirrors
//! `@t3tools/shared/keybindings`.
use crate::{
    p2::{invalid, Reply},
    store::State,
};
use serde_json::{json, Value};

const MAX_RULES: usize = 256;
const MAX_DEPTH: usize = 64;
const DEFAULTS: &str = include_str!("keybindings-defaults.json");
const COMMANDS: &str = include_str!("keybindings-commands.json");

pub fn handles(tag: &str) -> bool {
    matches!(tag, "server.upsertKeybinding" | "server.removeKeybinding")
}

pub fn call(s: &State, tag: &str, p: &Value) -> Reply {
    let rule = |v: &Value| -> Result<Value, Value> { validate(v).map_err(invalid) };
    let _guard = s.settings_lock.lock().unwrap();
    let mut custom = load_writable(s)?;
    let target = rule(p)?;
    if tag == "server.upsertKeybinding" {
        let replace = match p.get("replace").filter(|r| !r.is_null()) {
            Some(r) => Some(rule(r)?),
            None => None,
        };
        custom.retain(|entry| {
            !same(entry, &target) && replace.as_ref().is_none_or(|r| !same(entry, r))
        });
        custom.push(target);
        if custom.len() > MAX_RULES {
            custom.drain(..custom.len() - MAX_RULES);
        }
    } else {
        custom.retain(|entry| !same(entry, &target));
    }
    write(s, &custom)?;
    s.notify();
    Ok(json!({"keybindings":merge_defaults(compile_all(&custom)),"issues":[]}))
}

fn config_error(s: &State, detail: &str) -> Value {
    json!({"_tag":"KeybindingsConfigParseError","configPath":path(s),"detail":detail,"message":format!("Unable to parse keybindings config at {}: {detail}",path(s).display())})
}
fn path(s: &State) -> std::path::PathBuf {
    s.dir.join("keybindings.json")
}

fn same(a: &Value, b: &Value) -> bool {
    a["command"] == b["command"] && a["key"] == b["key"] && a.get("when").filter(|w| !w.is_null()) == b.get("when").filter(|w| !w.is_null())
}

/// A rule with trimmed strings and a known command, or why not.
fn validate(v: &Value) -> Result<Value, String> {
    let text = |k: &str, max: usize| -> Result<Option<String>, String> {
        match v.get(k).filter(|x| !x.is_null()) {
            None => Ok(None),
            Some(x) => {
                let t = x.as_str().ok_or_else(|| format!("{k} must be a string"))?.trim().to_owned();
                if t.is_empty() || t.chars().count() > max {
                    Err(format!("{k} is out of range"))
                } else {
                    Ok(Some(t))
                }
            }
        }
    };
    let key = text("key", 64)?.ok_or("key is required")?;
    let command = text("command", 64)?.ok_or("command is required")?;
    if !known_command(&command) {
        return Err(format!("unknown keybinding command: {command}"));
    }
    if parse_shortcut(&key).is_none() {
        return Err("key is not a valid shortcut".into());
    }
    let mut rule = json!({"key":key,"command":command});
    if let Some(when) = text("when", 256)? {
        if parse_when(&when).is_none() {
            return Err("when is not a valid expression".into());
        }
        rule["when"] = json!(when);
    }
    Ok(rule)
}

fn known_command(c: &str) -> bool {
    let list: Vec<String> = serde_json::from_str(COMMANDS).unwrap_or_default();
    if list.iter().any(|k| k == c) {
        return true;
    }
    c.strip_prefix("script.")
        .and_then(|r| r.strip_suffix(".run"))
        .is_some_and(|id| {
            !id.is_empty()
                && id.len() <= 24
                && id.as_bytes()[0].is_ascii_alphanumeric()
                && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

/// The user's rules for writing: a missing file is empty, a malformed one is an error that is not overwritten.
fn load_writable(s: &State) -> Result<Vec<Value>, Value> {
    match std::fs::read_to_string(path(s)) {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(Value::Array(a)) => Ok(a.iter().filter_map(|r| validate(r).ok()).collect()),
            Ok(_) | Err(_) => Err(config_error(s, "expected JSON array")),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(config_error(s, &e.to_string())),
    }
}

fn write(s: &State, rules: &[Value]) -> Result<(), Value> {
    crate::profile::atomic_json(&path(s), &json!(rules)).map_err(|e| config_error(s, &e.to_string()))
}

/// `(resolved keybindings, issues)` for `server.getConfig`.
pub fn resolved(s: &State) -> (Vec<Value>, Vec<Value>) {
    let mut issues = Vec::new();
    let mut custom = Vec::new();
    match std::fs::read_to_string(path(s)) {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(Value::Array(a)) => {
                for (i, r) in a.iter().enumerate() {
                    match validate(r).ok().and_then(|r| compile(&r)) {
                        Some(c) => custom.push(c),
                        None => issues.push(json!({"kind":"keybindings.invalid-entry","message":"Invalid keybinding entry","index":i})),
                    }
                }
            }
            _ => issues.push(json!({"kind":"keybindings.malformed-config","message":"Keybindings config must be a JSON array"})),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => issues.push(json!({"kind":"keybindings.malformed-config","message":e.to_string()})),
    }
    (merge_defaults(custom), issues)
}

fn compile_all(rules: &[Value]) -> Vec<Value> {
    let mut v: Vec<Value> = rules.iter().filter_map(compile).collect();
    if v.len() > MAX_RULES {
        v.drain(..v.len() - MAX_RULES);
    }
    v
}

fn merge_defaults(custom: Vec<Value>) -> Vec<Value> {
    let defaults: Vec<Value> = serde_json::from_str::<Vec<Value>>(DEFAULTS)
        .unwrap_or_default()
        .iter()
        .filter_map(compile)
        .collect();
    let mut merged: Vec<Value> = defaults
        .into_iter()
        .filter(|d| !custom.iter().any(|c| c["command"] == d["command"]))
        .collect();
    merged.extend(custom);
    if merged.len() > MAX_RULES {
        merged.drain(..merged.len() - MAX_RULES);
    }
    merged
}

fn compile(rule: &Value) -> Option<Value> {
    let shortcut = parse_shortcut(rule["key"].as_str()?)?;
    let mut out = json!({"command":rule["command"],"shortcut":shortcut});
    if let Some(when) = rule.get("when").and_then(Value::as_str) {
        out["whenAst"] = parse_when(when)?;
    }
    Some(out)
}

pub fn parse_shortcut(value: &str) -> Option<Value> {
    let lowered = value.to_lowercase();
    let mut tokens: Vec<String> = lowered.split('+').map(|t| t.trim().to_owned()).collect();
    let mut trailing = 0;
    while tokens.last().is_some_and(|t| t.is_empty()) {
        tokens.pop();
        trailing += 1;
    }
    if trailing > 0 {
        tokens.push("+".into());
    }
    if tokens.is_empty() || tokens.iter().any(|t| t.is_empty()) {
        return None;
    }
    let (mut key, mut meta, mut ctrl, mut shift, mut alt, mut modk) = (None::<String>, false, false, false, false, false);
    for t in tokens {
        match t.as_str() {
            "cmd" | "meta" => meta = true,
            "ctrl" | "control" => ctrl = true,
            "shift" => shift = true,
            "alt" | "option" => alt = true,
            "mod" => modk = true,
            other => {
                if key.is_some() {
                    return None;
                }
                key = Some(match other {
                    "space" => " ".into(),
                    "esc" => "escape".into(),
                    o => o.to_owned(),
                });
            }
        }
    }
    Some(json!({"key":key?,"metaKey":meta,"ctrlKey":ctrl,"shiftKey":shift,"altKey":alt,"modKey":modk}))
}

#[derive(Debug, PartialEq)]
enum Tok {
    And,
    Or,
    Not,
    Open,
    Close,
    Id(String),
}

fn tokenize(expr: &str) -> Option<Vec<Tok>> {
    let b = expr.as_bytes();
    let (mut i, mut out) = (0, Vec::new());
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
        } else if expr[i..].starts_with("&&") {
            out.push(Tok::And);
            i += 2;
        } else if expr[i..].starts_with("||") {
            out.push(Tok::Or);
            i += 2;
        } else if c == b'!' {
            out.push(Tok::Not);
            i += 1;
        } else if c == b'(' {
            out.push(Tok::Open);
            i += 1;
        } else if c == b')' {
            out.push(Tok::Close);
            i += 1;
        } else if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || matches!(b[i], b'_' | b'.' | b'-')) {
                i += 1;
            }
            out.push(Tok::Id(expr[start..i].to_owned()));
        } else {
            return None;
        }
    }
    Some(out)
}

pub fn parse_when(expr: &str) -> Option<Value> {
    let tokens = tokenize(expr)?;
    if tokens.is_empty() {
        return None;
    }
    let mut pos = 0;
    let node = or(&tokens, &mut pos, 0)?;
    (pos == tokens.len()).then_some(node)
}
fn primary(t: &[Tok], pos: &mut usize, depth: usize) -> Option<Value> {
    if depth > MAX_DEPTH {
        return None;
    }
    match t.get(*pos)? {
        Tok::Id(name) => {
            *pos += 1;
            Some(json!({"type":"identifier","name":name}))
        }
        Tok::Open => {
            *pos += 1;
            let inner = or(t, pos, depth + 1)?;
            if t.get(*pos) != Some(&Tok::Close) {
                return None;
            }
            *pos += 1;
            Some(inner)
        }
        _ => None,
    }
}
fn unary(t: &[Tok], pos: &mut usize, depth: usize) -> Option<Value> {
    let mut nots = 0;
    while t.get(*pos) == Some(&Tok::Not) {
        *pos += 1;
        nots += 1;
        if nots > MAX_DEPTH {
            return None;
        }
    }
    let mut node = primary(t, pos, depth)?;
    for _ in 0..nots {
        node = json!({"type":"not","node":node});
    }
    Some(node)
}
fn and(t: &[Tok], pos: &mut usize, depth: usize) -> Option<Value> {
    let mut left = unary(t, pos, depth)?;
    while t.get(*pos) == Some(&Tok::And) {
        *pos += 1;
        let right = unary(t, pos, depth)?;
        left = json!({"type":"and","left":left,"right":right});
    }
    Some(left)
}
fn or(t: &[Tok], pos: &mut usize, depth: usize) -> Option<Value> {
    let mut left = and(t, pos, depth)?;
    while t.get(*pos) == Some(&Tok::Or) {
        *pos += 1;
        let right = and(t, pos, depth)?;
        left = json!({"type":"or","left":left,"right":right});
    }
    Some(left)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shortcuts_and_when_expressions_parse() {
        let s = parse_shortcut("Mod+Shift+Esc").unwrap();
        assert_eq!((s["key"].as_str(), s["modKey"].as_bool(), s["shiftKey"].as_bool()), (Some("escape"), Some(true), Some(true)));
        assert_eq!(parse_shortcut("mod++").unwrap()["key"], "+");
        assert!(parse_shortcut("a+b").is_none());
        assert_eq!(parse_when("!a && (b || c)").unwrap()["type"], "and");
        assert!(parse_when("a &&").is_none());
        assert!(parse_when("a b").is_none());
    }
    #[test]
    fn every_default_compiles() {
        let defaults: Vec<Value> = serde_json::from_str(DEFAULTS).unwrap();
        assert!(defaults.len() > 50);
        assert!(defaults.iter().all(|d| compile(d).is_some() && known_command(d["command"].as_str().unwrap())));
    }
}
