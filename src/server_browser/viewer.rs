//! Viewer ownership and input release state for the existing framed stream transport.
use super::*;
#[derive(Default)]
pub(super) struct Pressed {
    keys: indexmap::IndexMap<String, (String, String)>,
    buttons: indexmap::IndexMap<String, (f64, f64)>,
}
pub(super) type Inputs = std::sync::Arc<std::sync::Mutex<Pressed>>;
pub(super) async fn dispatch(
    page: &Page,
    inputs: &Inputs,
    method: &str,
    params: Value,
) -> Result<Value, Value> {
    if method == "Viewer.history" {
        let history = page.command("Page.getNavigationHistory", json!({})).await?;
        let index =
            history["currentIndex"].as_i64().unwrap_or(0) + params["offset"].as_i64().unwrap_or(0);
        let Some(entry) = usize::try_from(index)
            .ok()
            .and_then(|index| history["entries"].get(index))
        else {
            return Ok(Value::Null);
        };
        return page
            .command(
                "Page.navigateToHistoryEntry",
                json!({"entryId":entry["id"]}),
            )
            .await;
    }
    let result = page.command(method, params.clone()).await?;
    let mut pressed = inputs.lock().unwrap();
    if method == "Input.dispatchKeyEvent" {
        let key = params["key"].as_str().unwrap_or("").to_string();
        let code = params["code"].as_str().unwrap_or("").to_string();
        let id = if code.is_empty() {
            key.clone()
        } else {
            code.clone()
        };
        if params["type"] == "keyUp" {
            pressed.keys.shift_remove(&id);
        } else {
            pressed.keys.insert(id, (key, code));
        }
    } else if method == "Input.dispatchMouseEvent" {
        let button = params["button"].as_str().unwrap_or("none");
        if button != "none" {
            if params["type"] == "mousePressed" {
                pressed
                    .buttons
                    .insert(button.into(), (number(&params["x"]), number(&params["y"])));
            } else if params["type"] == "mouseReleased" {
                pressed.buttons.shift_remove(button);
            }
        }
    }
    Ok(result)
}
pub(super) async fn release(page: Page, inputs: Inputs) -> Result<Value, Value> {
    let pressed = std::mem::take(&mut *inputs.lock().unwrap());
    for (_, (key, code)) in pressed.keys {
        let _ = page
            .command(
                "Input.dispatchKeyEvent",
                json!({"type":"keyUp","key":key,"code":code}),
            )
            .await;
    }
    for (button, (x, y)) in pressed.buttons {
        let _=page.command("Input.dispatchMouseEvent",json!({"type":"mouseReleased","button":button,"x":x,"y":y,"buttons":0,"clickCount":1})).await;
    }
    Ok(Value::Null)
}
pub(super) fn number(value: &Value) -> f64 {
    value
        .as_f64()
        .filter(|value| value.is_finite())
        .unwrap_or(0.0)
}
fn modifiers(value: &Value) -> f64 {
    let value = number(value);
    if value.fract() == 0.0 && (0.0..16.0).contains(&value) {
        value
    } else {
        0.0
    }
}
fn numeric(value: f64) -> String {
    ryu_js::Buffer::new().format(value).to_string()
}
fn sign(value: f64) -> i32 {
    if value > 0.0 {
        1
    } else if value < 0.0 {
        -1
    } else {
        0
    }
}
pub(super) fn latest_key(input: &Value) -> Option<String> {
    let modifiers = numeric(modifiers(&input["modifiers"]));
    if input["type"] == "mouse"
        && input["action"] == "move"
        && input["button"] == "none"
        && input["buttons"].as_f64() == Some(0.0)
    {
        Some(format!("hover:{modifiers}"))
    } else if input["type"] == "wheel" {
        Some(format!(
            "wheel:{}:{}:{modifiers}:{}:{}",
            numeric(number(&input["x"])),
            numeric(number(&input["y"])),
            sign(number(&input["deltaX"])),
            sign(number(&input["deltaY"]))
        ))
    } else {
        None
    }
}
pub(super) fn merge_wheel(previous: &Value, mut next: Value) -> Value {
    next["deltaX"] = json!(number(&previous["deltaX"]) + number(&next["deltaX"]));
    next["deltaY"] = json!(number(&previous["deltaY"]) + number(&next["deltaY"]));
    next
}
pub(super) fn control(
    snapshot: &crate::preview_control::runtime::Snapshot,
    viewer: &str,
    can_operate: bool,
    dialog: Value,
) -> Value {
    let controller = if snapshot.controller.as_deref() == Some(viewer) {
        "you"
    } else if snapshot.controller.is_some() {
        "another-viewer"
    } else if snapshot.agent_id.is_some() {
        "agent"
    } else {
        "unclaimed"
    };
    json!({"type":"control","_tag":"control","canOperate":can_operate,"controller":controller,"generation":snapshot.generation,"dialog":dialog})
}
pub(super) fn live(state: &crate::store::State, session: &Value) -> bool {
    session["sid"].as_str().is_some_and(|sid|state.db.lock().unwrap().query_row("SELECT EXISTS(SELECT 1 FROM auth_sessions WHERE session_id=? AND revoked_at IS NULL AND expires_at>?)",rusqlite::params![sid,crate::store::now_ms()],|row|row.get::<_,bool>(0)).unwrap_or(false))
}
