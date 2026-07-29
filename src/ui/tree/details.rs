use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Local};

use crate::model::{Branch, ConfiguredUpstream, RemoteRefEvidence};

pub fn relative_time(timestamp: i64, now: SystemTime) -> String {
    let now = now
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs() as i64;
    let seconds = now - timestamp;
    if seconds < -60 {
        return "clock?".into();
    }
    if seconds < 60 {
        "now".into()
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else if seconds < 172_800 {
        format!("{}h", seconds / 3600)
    } else {
        format!("{}d", seconds / 86_400)
    }
}

pub fn exact_time(timestamp: i64) -> String {
    DateTime::from_timestamp(timestamp, 0)
        .map(|value| {
            value
                .with_timezone(&Local)
                .format("%Y-%m-%d %H:%M:%S %:z")
                .to_string()
        })
        .unwrap_or_else(|| "unavailable".into())
}

pub fn detail(branch: &Branch) -> String {
    let pr = branch
        .pr
        .as_ref()
        .map(|pr| format!("PR #{} · {} · {}", pr.number, pr.status.label(), pr.title))
        .unwrap_or_else(|| "No matching PR".into());
    let worktree = branch
        .worktree
        .as_ref()
        .map(|path| format!("Worktree: {}", path.display()))
        .unwrap_or_else(|| "Worktree: not checked out".into());
    format!(
        "{}\nLast edited: {}\n{}\n{}\n{}\n{}\nRemote evidence uses local refs only · no fetch",
        branch.id,
        exact_time(branch.committed_at),
        worktree,
        pr,
        upstream_detail(&branch.configured_upstream),
        remote_detail(&branch.remote_ref),
    )
}

pub fn local_detail(branch: &Branch) -> String {
    let worktree = branch
        .worktree
        .as_ref()
        .map(|path| format!("Worktree: {}", path.display()))
        .unwrap_or_else(|| "Worktree: not checked out".into());
    format!(
        "{}\nLast edited: {}\n{}\n{}\n{}\nArchive view: local refs only · no fetch",
        branch.id,
        exact_time(branch.committed_at),
        worktree,
        upstream_detail(&branch.configured_upstream),
        remote_detail(&branch.remote_ref),
    )
}

pub fn evidence_source_footer(branch: &Branch) -> String {
    match &branch.remote_ref {
        RemoteRefEvidence::Contained {
            source_token,
            checked_at,
            ..
        }
        | RemoteRefEvidence::LocalOnly {
            source_token,
            checked_at,
        } => format!(
            "local-ref token {:016x} @ {}",
            source_token,
            system_time(*checked_at)
        ),
        RemoteRefEvidence::Unavailable {
            source_token,
            checked_at,
            ..
        } => format!(
            "local-ref token {} @ {}",
            source_token
                .map(|token| format!("{token:016x}"))
                .unwrap_or_else(|| "unknown".into()),
            system_time(*checked_at)
        ),
        RemoteRefEvidence::Checking => "local-ref evidence checking…".into(),
        RemoteRefEvidence::NotRequested => "local-ref evidence not checked".into(),
    }
}

fn upstream_detail(upstream: &ConfiguredUpstream) -> String {
    match upstream {
        ConfiguredUpstream::None => "Configured upstream: none".into(),
        ConfiguredUpstream::Equal { reference } => {
            format!("Configured upstream: {reference} (equal)")
        }
        ConfiguredUpstream::Ahead { reference, ahead } => {
            format!("Configured upstream: {reference} (ahead {ahead})")
        }
        ConfiguredUpstream::Behind { reference, behind } => {
            format!("Configured upstream: {reference} (behind {behind})")
        }
        ConfiguredUpstream::Diverged {
            reference,
            ahead,
            behind,
        } => format!("Configured upstream: {reference} (ahead {ahead}, behind {behind})"),
        ConfiguredUpstream::Gone { reference } => {
            format!("Configured upstream: {reference} (gone from local refs)")
        }
        ConfiguredUpstream::Unavailable { reference, reason } => format!(
            "Configured upstream: {} (unavailable: {reason})",
            reference.as_deref().unwrap_or("unknown")
        ),
    }
}

fn remote_detail(evidence: &RemoteRefEvidence) -> String {
    match evidence {
        RemoteRefEvidence::NotRequested => "Remote-ref evidence: not checked".into(),
        RemoteRefEvidence::Checking => "Remote-ref evidence: checking local refs…".into(),
        RemoteRefEvidence::Contained {
            reference,
            source_token,
            checked_at,
        } => format!(
            "Remote-ref evidence: contained in {reference}; source {:016x} @ {}",
            source_token,
            system_time(*checked_at)
        ),
        RemoteRefEvidence::LocalOnly {
            source_token,
            checked_at,
        } => format!(
            "Remote-ref evidence: local only; source {:016x} @ {}",
            source_token,
            system_time(*checked_at)
        ),
        RemoteRefEvidence::Unavailable {
            reason,
            source_token,
            checked_at,
        } => format!(
            "Remote-ref evidence: unavailable ({reason}); source {} @ {}",
            source_token
                .map(|token| format!("{token:016x}"))
                .unwrap_or_else(|| "unknown".into()),
            system_time(*checked_at)
        ),
    }
}

fn system_time(time: SystemTime) -> String {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| DateTime::from_timestamp(duration.as_secs() as i64, 0))
        .map(|value| {
            value
                .with_timezone(&Local)
                .format("%Y-%m-%d %H:%M:%S %:z")
                .to_string()
        })
        .unwrap_or_else(|| "unavailable".into())
}
