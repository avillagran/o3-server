use super::{Error, Result, Sink};
use serde_json::Value;
fn lines(v: &Value, out: &mut Sink) -> Result<()> {
    match v {
        Value::String(s) => out.add(s)?,
        Value::Array(a) => {
            for v in a {
                if let Some(s) = v.as_str() {
                    out.add(s)?
                }
            }
        }
        _ => {}
    }
    Ok(())
}
pub(super) fn notebook(data: &[u8], out: &mut Sink) -> Result<()> {
    let v: Value = serde_json::from_slice(data).map_err(|_| Error::Invalid)?;
    let cells = v["cells"].as_array().ok_or(Error::Invalid)?;
    let ansi =
        regex::Regex::new(r"\x1b(?:\[[0-?]*[ -/]*[@-~]|\][^\x07\x1b]*(?:\x07|\x1b\\)|[@-_])")
            .unwrap();
    for (index, c) in cells.iter().enumerate() {
        if index >= 10_000 {
            return Err(Error::Truncated);
        }
        if !c.is_object() {
            continue;
        }
        let kind = if c["cell_type"] == "code" {
            "code"
        } else {
            "markdown"
        };
        out.add(&format!("## Cell {} ({kind})\n", index + 1))?;
        lines(&c["source"], out)?;
        if let Some(outputs) = c["outputs"].as_array() {
            out.add("\nOutput:\n")?;
            for o in outputs {
                if !o.is_object() {
                    continue;
                }
                if o.get("text").is_some() {
                    lines(&o["text"], out)?
                } else if o["data"].get("text/plain").is_some() {
                    lines(&o["data"]["text/plain"], out)?
                } else if o["output_type"] == "error" {
                    lines(&o["ename"], out)?;
                    if o.get("evalue").is_some() {
                        out.add(": ")?;
                        lines(&o["evalue"], out)?
                    }
                    out.add("\n")?;
                    match &o["traceback"] {
                        Value::String(s) => {
                            out.add(&ansi.replace_all(s, ""))?;
                            out.add("\n")?
                        }
                        Value::Array(a) => {
                            for s in a.iter().filter_map(Value::as_str) {
                                out.add(&ansi.replace_all(s, ""))?;
                                out.add("\n")?
                            }
                        }
                        _ => {}
                    }
                }
                out.add("\n")?
            }
        }
        out.add("\n\n")?;
    }
    Ok(())
}
#[derive(Clone, Copy)]
struct State {
    skip: bool,
    uc: usize,
    encoding: &'static encoding_rs::Encoding,
}
fn symbol(s: &str) -> Option<&'static str> {
    Some(match s {
        "endash" => "–",
        "emdash" => "—",
        "enspace" => "\u{2002}",
        "emspace" => "\u{2003}",
        "qmspace" => "\u{2005}",
        "lquote" => "‘",
        "rquote" => "’",
        "ldblquote" => "“",
        "rdblquote" => "”",
        "bullet" => "•",
        "~" => "\u{a0}",
        "_" => "\u{2011}",
        "-" => "",
        _ => return None,
    })
}
fn emit(out: &mut Sink, pending: &mut Option<u16>, text: &str) -> Result<()> {
    if pending.take().is_some() {
        out.add("�")?
    }
    out.add(text)
}
pub(super) fn rtf(data: &[u8], out: &mut Sink) -> Result<()> {
    if !data.starts_with(b"{\\rtf") {
        return Err(Error::Invalid);
    }
    let mut state = State {
        skip: false,
        uc: 1,
        encoding: encoding_rs::WINDOWS_1252,
    };
    let mut stack = Vec::new();
    let mut fallback = 0usize;
    let mut i = 0;
    let mut pending = None;
    while i < data.len() {
        let ch = data[i];
        i += 1;
        match ch {
            b'{' => {
                fallback = 0;
                stack.push(state);
                if stack.len() > 128 {
                    return Err(Error::Truncated);
                }
            }
            b'}' => {
                fallback = 0;
                state = stack.pop().unwrap_or(state)
            }
            b'\\' => {
                let Some(&next) = data.get(i) else { break };
                if next == b'\''
                    && data
                        .get(i + 1..i + 3)
                        .is_some_and(|b| b.iter().all(u8::is_ascii_hexdigit))
                {
                    let byte =
                        u8::from_str_radix(std::str::from_utf8(&data[i + 1..i + 3]).unwrap(), 16)
                            .unwrap();
                    i += 3;
                    if fallback > 0 {
                        fallback -= 1
                    } else if !state.skip {
                        let bytes = [byte];
                        let text = state
                            .encoding
                            .decode_without_bom_handling_and_without_replacement(&bytes)
                            .ok_or(Error::Invalid)?;
                        emit(out, &mut pending, &text)?
                    }
                } else if next.is_ascii_alphabetic() {
                    let start = i;
                    while i < data.len() && data[i].is_ascii_alphabetic() {
                        i += 1
                    }
                    let control = std::str::from_utf8(&data[start..i]).unwrap();
                    let number = i;
                    if data.get(i) == Some(&b'-') {
                        i += 1
                    }
                    while i < data.len() && data[i].is_ascii_digit() {
                        i += 1
                    }
                    let value = std::str::from_utf8(&data[number..i])
                        .ok()
                        .and_then(|s| s.parse::<i64>().ok());
                    if data.get(i) == Some(&b' ') {
                        i += 1
                    }
                    if control == "bin" {
                        let n = value.filter(|n| *n >= 0).ok_or(Error::Invalid)? as usize;
                        i = i.saturating_add(n).min(data.len());
                    }
                    if fallback > 0 {
                        fallback -= 1;
                        continue;
                    }
                    if matches!(
                        control,
                        "fonttbl"
                            | "colortbl"
                            | "stylesheet"
                            | "info"
                            | "pict"
                            | "object"
                            | "fldinst"
                            | "header"
                            | "footer"
                    ) {
                        state.skip = true
                    }
                    match control {
                        "ansicpg" => {
                            let n = value.ok_or(Error::Invalid)?;
                            if n != 874 && !(1250..=1258).contains(&n) {
                                return Err(Error::Invalid);
                            }
                            state.encoding =
                                encoding_rs::Encoding::for_label(format!("windows-{n}").as_bytes())
                                    .ok_or(Error::Invalid)?;
                        }
                        "uc" => {
                            state.uc = value.filter(|n| *n >= 0).ok_or(Error::Invalid)? as usize
                        }
                        "u" if !state.skip => {
                            let unit = value.ok_or(Error::Invalid)? as u16;
                            if (0xdc00..=0xdfff).contains(&unit) && pending.is_some() {
                                let high = pending.take().unwrap();
                                let code = 0x10000
                                    + ((high as u32 - 0xd800) << 10)
                                    + (unit as u32 - 0xdc00);
                                out.add(&char::from_u32(code).unwrap_or('�').to_string())?
                            } else {
                                if pending.take().is_some() {
                                    out.add("�")?
                                }
                                if (0xd800..=0xdbff).contains(&unit) {
                                    pending = Some(unit)
                                } else {
                                    out.add(
                                        &char::from_u32(unit as u32).unwrap_or('�').to_string(),
                                    )?
                                }
                            }
                            fallback = state.uc;
                        }
                        "par" | "line" | "row" if !state.skip => emit(out, &mut pending, "\n")?,
                        "tab" | "cell" if !state.skip => emit(out, &mut pending, "\t")?,
                        _ if !state.skip => {
                            if let Some(s) = symbol(control) {
                                emit(out, &mut pending, s)?
                            }
                        }
                        _ => {}
                    }
                } else {
                    i += 1;
                    if fallback > 0 {
                        fallback -= 1
                    } else if next == b'*' {
                        state.skip = true
                    } else if !state.skip {
                        if matches!(next, b'\\' | b'{' | b'}') {
                            emit(out, &mut pending, &(next as char).to_string())?
                        } else if let Some(s) = symbol(std::str::from_utf8(&[next]).unwrap_or("")) {
                            emit(out, &mut pending, s)?
                        }
                    }
                }
            }
            _ if !state.skip && ch != b'\n' && ch != b'\r' => {
                if fallback > 0 {
                    fallback -= 1
                } else {
                    let bytes = [ch];
                    let text = state
                        .encoding
                        .decode_without_bom_handling_and_without_replacement(&bytes)
                        .ok_or(Error::Invalid)?;
                    emit(out, &mut pending, &text)?
                }
            }
            _ => {}
        }
    }
    if pending.is_some() {
        out.add("�")?
    }
    Ok(())
}
