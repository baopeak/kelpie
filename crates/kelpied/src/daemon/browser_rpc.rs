//! `browser.*`: agents using the browser panes the person sees (rules, arguments, rate and audit in `browser_agent.rs`).
//!
//! `browser.call` and `browser.policy` are on a session's scripting surface, restricted sessions included (`auth`), and
//! `authorize` puts the caller's own id into `browser.call`'s `session_id`. `browser.audit` is the user's.

use super::*;
use crate::browser_agent::{self as agent, Class};

/// The most audit lines `browser.audit` gives.
const MAX_AUDIT: u64 = 500;

impl Daemon {
    /// `browser.policy`: what agents may do with browser panes now, and whether sub-sessions are allowed (the MCP server lists each
    /// group of tools by it).
    pub(super) fn browser_policy(&self) -> Value {
        let p = self.browser.policy.read();
        json!({"mode": p.mode.as_str(), "eval": p.eval, "sub_sessions": self.subs.policy.read().mode.as_str()})
    }

    /// `browser.call {tool, args}` from a program in a session: checked here (policy, rate, arguments), then asked of the app's window,
    /// which answers later. A call that acts or runs a script and gets that far is audited, whatever the window does with it; one refused
    /// here is not (a refusal loop must not push the record of what was done out of the audit).
    pub(super) fn browser_call(self: &Arc<Self>, tx: &ClientTx, id: &Value, params: &Value) -> Result<Option<Value>> {
        let sid = params.get("session_id").and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or_else(|| anyhow!("browser.call is for programs running in a Kelpie session"))?.to_string();
        let tool = params.get("tool").and_then(Value::as_str).unwrap_or_default().to_string();
        let class = agent::classify(&tool).ok_or_else(|| anyhow!(agent::unknown_tool(&tool)))?;
        let spec = self.by_id(&sid).ok_or_else(|| anyhow!("the calling session no longer exists"))?.spec();
        let label = label_of(&spec);
        let policy = self.browser.policy.read().clone();
        agent::gate(&policy, &tool).map_err(|e| anyhow!(e))?;
        anyhow::ensure!(self.browser.rate.lock().take(&sid, Instant::now()), agent::TOO_MANY);
        // The reason may name what was wrong, never the value.
        let args = agent::check_args(&tool, params.get("args").unwrap_or(&Value::Null)).map_err(|e| anyhow!(e))?;
        let limit = agent::limit(&tool, &args);
        // A restricted session is offered only the panes opened for it, not those the person shared.
        let session = json!({"id": spec.id, "name": spec.name, "kind": spec.kind, "cwd": spec.cwd, "parent": spec.parent, "restricted": spec.restricted});
        let request = agent::request(session, &tool, &args, &policy);
        let (me, tx, id) = (self.clone(), tx.clone(), id.clone());
        tokio::spawn(async move {
            let result = agent::forward(&me.providers, request, limit).await;
            if class != Class::Read {
                let line = agent::audit_line(now_millis(), &sid, &label, &tool, &args, result.as_ref().ok(), result.as_ref().err().map(String::as_str));
                let audit = me.browser.audit.clone();
                // Written before the answer: what an agent was told it did is already on record.
                let _ = tokio::task::spawn_blocking(move || audit.append(&line)).await;
            }
            let _ = tx.send(json_out(&rpc_answer(&id, result.map_err(|e| anyhow!(e)))));
        });
        Ok(None)
    }

    /// `browser.audit {limit}` (the user's): the last lines, newest last, at most 500 (default 50).
    pub(super) fn browser_audit(&self, tx: &ClientTx, id: &Value, params: &Value) {
        let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(50).clamp(1, MAX_AUDIT) as usize;
        let (audit, tx, id) = (self.browser.audit.clone(), tx.clone(), id.clone());
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
