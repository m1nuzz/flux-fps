//! Wire protocol for the elevated helper's Named Pipe bridge.
//!
//! The resident UI process runs without elevation. Privileged operations are
//! delegated to `flux-fps.exe --helper`, which is started elevated by a Task
//! Scheduler logon task. Requests and replies are single UTF-8 lines (one pipe
//! message per line). The grammar is deliberately tiny so the helper never
//! parses untrusted structured data.

/// Semantic version of the pipe protocol. Bumped on incompatible changes.
pub const PROTOCOL_VERSION: u32 = 1;

/// Hard cap for a single request or reply line.
pub const MAX_MESSAGE_BYTES: usize = 4096;

/// Prefix of the per-user pipe. The current user's SID is appended, so two
/// logged-on users can never address each other's helper.
pub const PIPE_NAME_PREFIX: &str = "flux-fps-helper-";

/// Builds the full Win32 pipe path for a user SID string.
///
/// SID strings contain only ASCII digits and hyphens; anything else is
/// dropped so an unexpected value cannot escape the pipe namespace.
pub fn pipe_path(user_sid: &str) -> String {
    let safe: String = user_sid
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    format!(r"\\.\pipe\{PIPE_NAME_PREFIX}{safe}")
}

/// Operations that require elevation. Every variant is idempotent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelperRequest {
    /// Handshake: reports protocol version and elevation status.
    Ping,
    /// Purge priority-0 standby pages (command 4 only).
    PurgeStandbyList,
    /// Apply background CPU Set isolation to one process.
    ApplyIsolation { pid: u32 },
    /// Remove background isolation from one process.
    RemoveIsolation { pid: u32 },
    /// Remove isolation from every process the helper has touched.
    RestoreAll,
    /// Ask the helper to stop its accept loop.
    Shutdown,
}

/// One-line response to a [`HelperRequest`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelperReply {
    /// Handshake answer.
    Pong { version: u32, elevated: bool },
    /// The operation completed (or was already in the desired state).
    Ok,
    /// The operation failed; the message is sanitized before transmission.
    Error(String),
}

impl HelperRequest {
    /// Encodes the request as one pipe message (no trailing newline).
    pub fn encode(&self) -> String {
        match self {
            Self::Ping => "ping".into(),
            Self::PurgeStandbyList => "purge".into(),
            Self::ApplyIsolation { pid } => format!("isolate:{pid}"),
            Self::RemoveIsolation { pid } => format!("release:{pid}"),
            Self::RestoreAll => "restore".into(),
            Self::Shutdown => "shutdown".into(),
        }
    }

    /// Decodes one pipe message. Unknown verbs and malformed pids are rejected.
    pub fn decode(line: &str) -> Option<Self> {
        let line = line.trim();
        let (verb, arg) = line.split_once(':').unwrap_or((line, ""));
        match verb {
            "ping" if arg.is_empty() => Some(Self::Ping),
            "purge" if arg.is_empty() => Some(Self::PurgeStandbyList),
            "isolate" => arg
                .parse::<u32>()
                .ok()
                .map(|pid| Self::ApplyIsolation { pid }),
            "release" => arg
                .parse::<u32>()
                .ok()
                .map(|pid| Self::RemoveIsolation { pid }),
            "restore" if arg.is_empty() => Some(Self::RestoreAll),
            "shutdown" if arg.is_empty() => Some(Self::Shutdown),
            _ => None,
        }
    }
}

impl HelperReply {
    /// Encodes the reply as one pipe message (no trailing newline).
    pub fn encode(&self) -> String {
        match self {
            Self::Pong { version, elevated } => {
                format!("pong:{version}:{}", u8::from(*elevated))
            }
            Self::Ok => "ok".into(),
            Self::Error(message) => format!("err:{}", sanitize(message)),
        }
    }

    /// Decodes one pipe message.
    pub fn decode(line: &str) -> Option<Self> {
        let line = line.trim();
        if line == "ok" {
            return Some(Self::Ok);
        }
        match line.split_once(':') {
            Some(("pong", tail)) => {
                let (version, elevated) = tail.split_once(':')?;
                Some(Self::Pong {
                    version: version.parse().ok()?,
                    elevated: match elevated {
                        "1" => true,
                        "0" => false,
                        _ => return None,
                    },
                })
            }
            Some(("err", message)) => Some(Self::Error(sanitize(message))),
            _ => None,
        }
    }
}

/// Keeps a message on a single line and strips the field separator.
fn sanitize(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() || c == ':' { ' ' } else { c })
        .take(256)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipe_path_is_per_user_and_namespace_safe() {
        let path = pipe_path("S-1-5-21-1234-5678-9012-1001");
        assert_eq!(
            path,
            r"\\.\pipe\flux-fps-helper-S-1-5-21-1234-5678-9012-1001"
        );
        assert_eq!(
            pipe_path("S-1-5-18\\pipe/evil"),
            r"\\.\pipe\flux-fps-helper-S-1-5-18pipeevil"
        );
    }

    #[test]
    fn requests_round_trip() {
        let requests = [
            HelperRequest::Ping,
            HelperRequest::PurgeStandbyList,
            HelperRequest::ApplyIsolation { pid: 42 },
            HelperRequest::RemoveIsolation { pid: 9001 },
            HelperRequest::RestoreAll,
            HelperRequest::Shutdown,
        ];
        for request in requests {
            assert_eq!(HelperRequest::decode(&request.encode()), Some(request));
        }
    }

    #[test]
    fn malformed_requests_are_rejected() {
        for line in [
            "",
            "unknown",
            "isolate:",
            "isolate:-1",
            "isolate:4294967296",
            "ping:extra",
            "release:12:34",
        ] {
            assert_eq!(HelperRequest::decode(line), None, "accepted {line:?}");
        }
    }

    #[test]
    fn replies_round_trip() {
        let replies = [
            HelperReply::Pong {
                version: PROTOCOL_VERSION,
                elevated: true,
            },
            HelperReply::Pong {
                version: PROTOCOL_VERSION,
                elevated: false,
            },
            HelperReply::Ok,
            HelperReply::Error("win32 error 5".into()),
        ];
        for reply in replies {
            assert_eq!(HelperReply::decode(&reply.encode()), Some(reply));
        }
    }

    #[test]
    fn error_messages_cannot_break_framing() {
        let reply = HelperReply::Error("bad\nline:with\rcontrols".into()).encode();
        assert!(!reply.contains('\n'));
        assert!(!reply.contains('\r'));
        assert!(reply.starts_with("err:"));
        assert_eq!(
            HelperReply::decode(&reply),
            Some(HelperReply::Error("bad line with controls".into()))
        );
    }
}
