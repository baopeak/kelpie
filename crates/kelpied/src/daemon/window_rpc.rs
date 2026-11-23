//! `window.*`: agents using the windows of programs they started (rules, arguments, rate, audit and the helper run in `window_agent.rs`).
//!
//! `window.call` and `window.policy` are on the scripting surface of a session that may script other sessions, and not on a restricted
//! session's (`auth`): the tools add power to a session that is limited, so a limited one does not get them. `authorize` puts the
//! caller's own id into `window.call`'s `session_id`. `window.audit` is the user's.

use super::*;
use crate::window_agent as agent;

/// The most audit lines `window.audit` gives.
const MAX_AUDIT: u64 = 500;

impl Daemon {
    /// `window.policy`: whether agents may use windows now (the MCP server lists the tools by it). Always `off` where the helper does
    /// not run.
    pub(super) fn window_policy(&self) -> Value {
        let p = self.window.policy.read();
        let mode = if self.window.supported { p.mode } else { agent::Mode::Off };
        json!({"mode": mode.as_str(), "visible": p.visible})
    }

    /// `window.call {tool, args}` from a program in a session: checked and carried out by the helper (`agent::execute`), which answers
    /// later. A restricted session is refused here too, whatever its method list says.
    pub(super) fn window_call(self: &Arc<Self>, tx: &ClientTx, id: &Value, params: &Value) -> Result<Option<Value>> {
        let sid = params.get("session_id").and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or_else(|| anyhow!("window.call is for programs running in a Kelpie session"))?.to_string();
        let tool = params.get("tool").and_then(Value::as_str).unwrap_or_default().to_string();
        let spec = self.by_id(&sid).ok_or_else(|| anyhow!("the calling session no longer exists"))?.spec();
        anyhow::ensure!(!spec.restricted, "window.call is not available to a restricted session");
        let label = label_of(&spec);
        let args = params.get("args").cloned().unwrap_or(Value::Null);
        let (me, tx, id) = (self.clone(), tx.clone(), id.clone());
        tokio::spawn(async move {
            let state = me.window.clone();
            let result = tokio::task::spawn_blocking(move || agent::execute(&state, &sid, &label, &tool, &args, now_millis())).await.unwrap_or_else(|e| Err(format!("the window tool stopped: {e}")));
            let _ = tx.send(json_out(&rpc_answer(&id, result.map_err(|e| anyhow!(e)))));
        });
        Ok(None)
    }

    /// `window.audit {limit}` (the user's): the last lines, newest last, at most 500 (default 50).
    pub(super) fn window_audit(&self, tx: &ClientTx, id: &Value, params: &Value) {
        let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(50).clamp(1, MAX_AUDIT) as usize;
        let (audit, tx, id) = (self.window.audit.clone(), tx.clone(), id.clone());
        let answer = move || {
            let _ = tx.send(json_out(&rpc_answer(&id, Ok(json!({"entries": audit.tail(limit)})))));
        };
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => drop(runtime.spawn_blocking(answer)),
            Err(_) => answer(),
        }
    }
}

#[cfg(test)]
mod tests;
