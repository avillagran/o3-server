//! Shared, bounded repository cloning; cloning alone never registers a project.
use crate::{
    mcp_access::Authority,
    paths,
    proc::{self, Opts},
    store::State,
};
use serde_json::{json, Value};
type Reply = Result<Value, Value>;
fn fail(detail: impl Into<String>) -> Value {
    let detail = detail.into();
    json!({"_tag":"SourceControlRepositoryError","provider":"unknown","operation":"cloneRepository","message":detail,"detail":detail})
}
fn redacted(raw: &str) -> String {
    url::Url::parse(raw)
        .ok()
        .and_then(|mut u| {
            u.set_username("").ok()?;
            u.set_password(None).ok()?;
            Some(u.to_string())
        })
        .unwrap_or_else(|| raw.to_owned())
}
fn https_fallback(raw: &str, repository: &Value) -> Option<String> {
    if repository["sshUrl"] == raw {
        return repository["url"].as_str().map(str::to_owned);
    }
    if let Ok(url) = url::Url::parse(raw) {
        if url.scheme() == "ssh" {
            return Some(format!(
                "https://{}/{}",
                url.host_str()?,
                url.path().trim_start_matches('/')
            ));
        }
    }
    let (user_host, path) = raw.split_once(':')?;
    let (_, host) = user_host.split_once('@')?;
    (!host.is_empty() && !path.is_empty() && !host.contains('/'))
        .then(|| format!("https://{host}/{path}"))
}
fn ssh_rejected(out: &proc::Out) -> bool {
    let message = out.stderr.to_ascii_lowercase();
    [
        "permission denied",
        "publickey",
        "authentication failed",
        "host key verification failed",
    ]
    .iter()
    .any(|s| message.contains(s))
}
pub(crate) async fn clone(s: &State, p: &Value, authority: Option<&Authority>) -> Reply {
    if let Some(a) = authority {
        a.environment(s)?;
    }
    if p.get("protocol").is_some()
        && !matches!(p["protocol"].as_str(), Some("auto" | "ssh" | "https"))
    {
        return Err(fail("Invalid clone protocol."));
    }
    let dest = p["destinationPath"]
        .as_str()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| fail("Choose a destination path before cloning."))?;
    let dest = std::path::absolute(paths::normalize(&paths::expand_home(dest)))
        .map_err(|e| fail(e.to_string()))?;
    // Serialize clones to the same destination, including while it does not exist yet.
    let _gate = proc::lock_repo(&dest).await;
    let mut repository = Value::Null;
    let mut remote = p["remoteUrl"]
        .as_str()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned);
    if let (Some(provider), Some(repo)) = (p["provider"].as_str(), p["repository"].as_str()) {
        if provider != "github" {
            return Err(fail(format!("Repository lookup is unavailable for provider {provider}. Pass remoteUrl to clone directly.")));
        }
        let repo = repo.trim();
        if repo.starts_with('-') || repo.chars().any(char::is_control) {
            return Err(fail("Invalid repository name."));
        }
        let output = proc::run(
            "gh",
            &s.dir,
            &["repo", "view", repo, "--json", "nameWithOwner,url,sshUrl"],
            Opts::NETWORK,
            &[],
        )
        .await
        .map_err(|e| fail(format!("Repository lookup failed: {e:?}")))?;
        if !output.ok() {
            return Err(fail("Repository lookup failed."));
        }
        let info: Value = serde_json::from_slice(&output.stdout)
            .map_err(|_| fail("Invalid repository lookup response."))?;
        if ["nameWithOwner", "url", "sshUrl"]
            .iter()
            .any(|key| !info[key].as_str().is_some_and(|v| !v.trim().is_empty()))
        {
            return Err(fail("Invalid repository lookup response."));
        }
        let https = info["url"]
            .as_str()
            .ok_or_else(|| fail("Repository clone URL is missing."))?;
        remote = Some(
            if p["protocol"] != "https" {
                info["sshUrl"]
                    .as_str()
                    .ok_or_else(|| fail("Repository SSH URL is missing."))?
            } else {
                https
            }
            .to_owned(),
        );
        repository = json!({"provider":provider,"nameWithOwner":info["nameWithOwner"],"url":https,"sshUrl":info["sshUrl"]});
    }
    let mut remote =
        remote.ok_or_else(|| fail("Enter a repository path or clone URL before cloning."))?;
    if remote.starts_with('-') || remote.chars().any(char::is_control) {
        return Err(fail("The remote URL is invalid."));
    }
    if let Some(a) = authority {
        a.environment(s)?;
    }
    match std::fs::read_dir(&dest) {
        Ok(mut entries) => {
            if entries.next().is_some() {
                return Err(fail("Destination path already exists and is not empty."));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {
            return Err(fail(
                "Destination path already exists and is not a directory.",
            ))
        }
    }
    let parent = dest
        .parent()
        .ok_or_else(|| fail("Destination has no parent directory."))?;
    std::fs::create_dir_all(parent).map_err(|e| fail(e.to_string()))?;
    let mut out = proc::git(
        parent,
        &[
            "clone",
            "--progress",
            "--",
            &remote,
            &dest.to_string_lossy(),
        ],
        Opts::NETWORK,
    )
    .await
    .map_err(|e| fail(format!("Clone failed: {e:?}")))?;
    if !out.ok() && p["protocol"] != "ssh" && ssh_rejected(&out) {
        if let Some(https) = https_fallback(&remote, &repository) {
            if let Some(a) = authority {
                a.environment(s)?;
            }
            out = proc::git(
                parent,
                &["clone", "--progress", "--", &https, &dest.to_string_lossy()],
                Opts::NETWORK,
            )
            .await
            .map_err(|e| fail(format!("HTTPS clone failed: {e:?}")))?;
            remote = https;
        }
    }
    if !out.ok() {
        return Err(fail(
            "Git could not clone the repository. The destination has not been registered.",
        ));
    }
    Ok(json!({"cwd":dest,"remoteUrl":redacted(&remote),"repository":repository}))
}
