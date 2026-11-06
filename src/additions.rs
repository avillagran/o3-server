//! RPCs added upstream after the pinned 3726 contract (docs/t3-contract-additions.json) whose
//! features o3-server does not provide. Each answers with upstream's own typed result or error so
//! newer clients degrade as upstream does when the feature is unavailable; the matching
//! capability flags stay unadvertised.
use serde_json::{json, Value};

pub fn handles(tag: &str) -> bool {
    matches!(
        tag,
        "orchestration.getNativeChildWork"
            | "sourceControl.appAccess"
            | "sourceControl.signInStart"
            | "sourceControl.signInWait"
            | "sourceControl.signInCancel"
            | "peerLinks.list"
            | "peerLinks.link"
            | "peerLinks.unlink"
            | "peerLinks.answerRequest"
            | "preview.stopLocalServer"
            | "server.getNpmRelease"
            | "server.prepareTriage"
            | "projects.createFromThread"
    )
}

const PEER_LINKS_OFF: &str =
    "Linked environments is off on this machine. Turn it on in Settings → Labs.";

pub fn call(tag: &str, p: &Value) -> Result<Value, Value> {
    match tag {
        // Only provider-native (Antigravity) subagents have readable child work upstream; every
        // other driver answers with no children. o3 has no such driver.
        "orchestration.getNativeChildWork" => Ok(json!({"children":[]})),
        // Without an app sign-in driver upstream reports signed-out.
        "sourceControl.appAccess" => Ok(json!({"status":"signed-out"})),
        "sourceControl.signInStart" | "sourceControl.signInWait" | "sourceControl.signInCancel" => {
            let provider = p["provider"].as_str().unwrap_or("github");
            let detail =
                format!("This server cannot sign in to {provider}. Use a token or the host's CLI.");
            Err(
                json!({"_tag":"SourceControlSignInError","provider":provider,"host":p["host"].as_str().unwrap_or(""),"reason":"unsupported","detail":detail,"message":detail}),
            )
        }
        "peerLinks.list" => Ok(json!({"links":[]})),
        "peerLinks.answerRequest" => Err(
            json!({"_tag":"PeerLinkRequestError","reason":"not_found","message":"The link request was not found."}),
        ),
        "peerLinks.link" | "peerLinks.unlink" => {
            Err(json!({"_tag":"PeerLinkError","reason":"disabled","message":PEER_LINKS_OFF}))
        }
        "preview.stopLocalServer" => Err(
            json!({"_tag":"PreviewStopLocalServerError","reason":"unsupported","message":"Stopping local servers is not supported on this machine."}),
        ),
        "server.getNpmRelease" => Err(
            json!({"_tag":"ServerNpmReleaseError","reason":"o3-server is not distributed through npm","message":"Could not check the configured npm registry: o3-server is not distributed through npm"}),
        ),
        "server.prepareTriage" => Err(
            json!({"_tag":"ServerPrepareTriageError","message":"Could not write the triage session files."}),
        ),
        "projects.createFromThread" => Err(
            json!({"_tag":"OrchestrationDispatchCommandError","message":"Only a No project thread with its own folder can become a project."}),
        ),
        _ => Err(json!({"_tag":"RpcNotImplemented"})),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_addition_is_scoped_and_answers_with_its_upstream_tag() {
        let doc: serde_json::Value =
            serde_json::from_str(include_str!("../docs/t3-contract-additions.json")).unwrap();
        for (tag, meta) in doc["methods"].as_object().unwrap() {
            assert_eq!(
                crate::scopes::required(tag),
                meta["scope"].as_str(),
                "{tag} must keep its upstream scope"
            );
        }
        for tag in [
            "peerLinks.link",
            "preview.stopLocalServer",
            "sourceControl.signInStart",
        ] {
            let err = super::call(
                tag,
                &serde_json::json!({"provider":"github","host":"github.com"}),
            )
            .unwrap_err();
            assert_ne!(err["_tag"], "RpcNotImplemented", "{tag}");
        }
        assert_eq!(
            super::call(
                "orchestration.getNativeChildWork",
                &serde_json::json!({"threadId":"t"})
            )
            .unwrap(),
            serde_json::json!({"children":[]})
        );
    }
}
