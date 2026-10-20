//! Serial Host V1 server. stdout belongs exclusively to framed responses.
use ahu_host_api::protocol::{
    self, ErrorCode, Failure, Hello, RemoteError, Request, Response, Success,
};
use ahu_host_api::v1::{Host, WorkspaceCreationError, WorkspaceRequest};
use ahu_host_api::{Error, Result};
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub const PLUGIN_ID: &str = "cmux-local";

// Optional arguments must be present, even when their value is null.
fn required_option<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Option<Id>, D::Error> {
    Option::deserialize(d)
}

#[derive(Deserialize)]
#[serde(try_from = "String")]
struct Id(String);

impl TryFrom<String> for Id {
    type Error = &'static str;
    fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
        if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
            Err("invalid host identifier")
        } else {
            Ok(Self(value))
        }
    }
}

impl std::ops::Deref for Id {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

#[derive(Deserialize)]
#[serde(try_from = "String")]
struct AbsolutePath(PathBuf);

impl TryFrom<String> for AbsolutePath {
    type Error = &'static str;
    fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
        if !Path::new(&value).is_absolute() || value.contains('\0') {
            Err("host paths must be absolute UTF-8 paths without NUL")
        } else {
            Ok(Self(value.into()))
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HelloParams {
    protocol_versions: Vec<u32>,
    host_api: u32,
    plugin_id: String,
    required_capabilities: Vec<String>,
}

#[derive(Deserialize)]
#[serde(
    tag = "method",
    content = "params",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum Operation {
    ListGroups {
        #[serde(deserialize_with = "required_option")]
        window_id: Option<Id>,
    },
    FindGroup {
        group_id: Id,
        #[serde(deserialize_with = "required_option")]
        window_id: Option<Id>,
    },
    CurrentWindow {},
    WindowForWorkspace {
        workspace_id: Id,
    },
    AddWorkspaceToGroup {
        group_id: Id,
        workspace_id: Id,
        window_id: Id,
    },
    CurrentWorkspace {},
    CreateAnchorWorkspace {
        group_id: Id,
        title: String,
        cwd: AbsolutePath,
    },
    SetAnchor {
        group_id: Id,
        workspace_id: Id,
    },
    CreateGroup {
        name: String,
        cwd: AbsolutePath,
    },
    ExpandGroup {
        group_id: Id,
    },
    CreateTaskWorkspace {
        group_id: Id,
        #[serde(deserialize_with = "required_option")]
        window_id: Option<Id>,
        title: String,
        summary: String,
        cwd: AbsolutePath,
        startup_command: String,
        focus: bool,
    },
    OpenMarkdown {
        path: AbsolutePath,
        focus: bool,
    },
    SelectWorkspace {
        workspace_id: Id,
    },
    CloseWorkspace {
        workspace_id: Id,
    },
    SetStatus {
        workspace_id: Id,
        value: String,
    },
    SetAgentMetadata {
        workspace_id: Id,
        agent: String,
        harness: String,
        model: String,
    },
    Workspaces {},
    WorkspacesInWindow {
        #[serde(deserialize_with = "required_option")]
        window_id: Option<Id>,
    },
}

struct DispatchError(RemoteError);

fn remote_error(code: ErrorCode, message: impl std::fmt::Display) -> RemoteError {
    RemoteError {
        code,
        message: message.to_string().chars().take(512).collect(),
    }
}

impl From<Error> for DispatchError {
    fn from(error: Error) -> Self {
        Self(remote_error(ErrorCode::Failed, error))
    }
}

impl From<WorkspaceCreationError> for DispatchError {
    fn from(error: WorkspaceCreationError) -> Self {
        let code = match &error {
            WorkspaceCreationError::NotCreated(_) => ErrorCode::NotCreated,
            WorkspaceCreationError::OutcomeUnknown(_) => ErrorCode::OutcomeUnknown,
        };
        Self(remote_error(code, error))
    }
}

fn dispatch(host: &impl Host, operation: Operation) -> std::result::Result<Value, DispatchError> {
    use Operation::*;
    Ok(match operation {
        ListGroups { window_id } => {
            json!({"complete":true, "groups":host.list_groups(window_id.as_deref())?})
        }
        FindGroup {
            group_id,
            window_id,
        } => json!(host.find_group(&group_id, window_id.as_deref())?),
        CurrentWindow {} => json!(host.current_window()?),
        WindowForWorkspace { workspace_id } => json!(host.window_for_workspace(&workspace_id)?),
        AddWorkspaceToGroup {
            group_id,
            workspace_id,
            window_id,
        } => json!(host.add_workspace_to_group(&group_id, &workspace_id, &window_id)?),
        CurrentWorkspace {} => json!(host.current_workspace()?),
        CreateAnchorWorkspace {
            group_id,
            title,
            cwd,
        } => json!(host.create_anchor_workspace(&group_id, &title, &cwd.0)?),
        SetAnchor {
            group_id,
            workspace_id,
        } => json!(host.set_anchor(&group_id, &workspace_id)?),
        CreateGroup { name, cwd } => json!(host.create_group(&name, &cwd.0)?),
        ExpandGroup { group_id } => json!(host.expand_group(&group_id)?),
        CreateTaskWorkspace {
            group_id,
            window_id,
            title,
            summary,
            cwd,
            startup_command,
            focus,
        } => json!(host.create_task_workspace(&WorkspaceRequest {
            group_id: &group_id,
            window_id: window_id.as_deref(),
            title: &title,
            summary: &summary,
            cwd: &cwd.0,
            startup_command: &startup_command,
            focus,
        })?),
        OpenMarkdown { path, focus } => json!(host.open_markdown(&path.0, focus)?),
        SelectWorkspace { workspace_id } => json!(host.select_workspace(&workspace_id)?),
        CloseWorkspace { workspace_id } => json!(host.close_workspace(&workspace_id)?),
        SetStatus {
            workspace_id,
            value,
        } => json!(host.set_status(&workspace_id, &value)?),
        SetAgentMetadata {
            workspace_id,
            agent,
            harness,
            model,
        } => json!(host.set_agent_metadata(&workspace_id, &agent, &harness, &model)?),
        Workspaces {} => json!({"complete":true, "workspaces":host.workspaces()?}),
        // V1 null scope means all windows, unlike the native workspace.list RPC.
        WorkspacesInWindow { window_id } => json!({"complete":true, "workspaces":match window_id {
            Some(window) => host.workspaces_in_window(Some(&window))?,
            None => host.workspaces()?,
        }}),
    })
}

fn read_request(input: &mut impl Read) -> Result<Option<Request>> {
    let mut header = [0; 4];
    // EOF is clean only between frames; partial headers/bodies are fatal.
    loop {
        match input.read(&mut header[..1]) {
            Ok(0) => return Ok(None),
            Ok(_) => break,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        }
    }
    input.read_exact(&mut header[1..])?;
    let length = u32::from_be_bytes(header) as usize;
    if length == 0 || length > protocol::MAX_FRAME_BYTES {
        return Err(Error::new("invalid Host V1 frame length"));
    }
    let mut body = vec![0; length];
    input.read_exact(&mut body)?;
    Ok(Some(protocol::from_slice(&body)?))
}

// Bound serialization itself, and never send a partial frame for an oversized result.
struct FrameBuffer(Vec<u8>);
impl Write for FrameBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > protocol::MAX_FRAME_BYTES - self.0.len() {
            return Err(std::io::Error::other(
                "Host V1 response exceeds frame limit",
            ));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn write_response(output: &mut impl Write, response: Response) -> Result<()> {
    let mut frame = FrameBuffer(Vec::new());
    serde_json::to_writer(&mut frame, &response)?;
    output.write_all(&(frame.0.len() as u32).to_be_bytes())?;
    output.write_all(&frame.0)?;
    output.flush()?;
    Ok(())
}

/// Connect only after a valid hello. Protocol violations terminate the stream;
/// backend errors are framed without retrying or inferring rollback.
pub fn serve<H: Host>(
    input: &mut impl Read,
    output: &mut impl Write,
    connect: impl FnOnce() -> Result<H>,
) -> Result<()> {
    let Some(request) = read_request(input)? else {
        return Ok(());
    };
    if request.protocol_version != protocol::VERSION || request.id != 1 || request.method != "hello"
    {
        return Err(Error::new("Host V1 requires hello with version 1 and id 1"));
    }
    let params: HelloParams = serde_json::from_value(request.params)?;
    let required: BTreeSet<_> = params
        .required_capabilities
        .iter()
        .map(String::as_str)
        .collect();
    if !params.protocol_versions.contains(&protocol::VERSION)
        || params.host_api != 1
        || params.plugin_id != PLUGIN_ID
        || required.len() != params.required_capabilities.len()
        || !required.iter().all(|m| protocol::METHODS.contains(m))
    {
        return Err(Error::new("incompatible Host V1 hello"));
    }
    let host = match connect().and_then(|host| {
        host.check_capabilities()?;
        Ok(host)
    }) {
        Ok(host) => host,
        Err(error) => {
            write_response(
                output,
                Response::Failure(Failure {
                    protocol_version: protocol::VERSION,
                    id: request.id,
                    error: remote_error(ErrorCode::Failed, error),
                }),
            )?;
            return Err(Error::new("cmux prerequisite check failed"));
        }
    };
    let inherited_workspace = host.inherited_workspace();
    if let Some(id) = &inherited_workspace {
        Id::try_from(id.clone()).map_err(Error::new)?;
    }
    write_response(
        output,
        Response::Success(Success {
            protocol_version: protocol::VERSION,
            id: request.id,
            result: json!(Hello {
                protocol_version: protocol::VERSION,
                host_api: 1,
                plugin_id: PLUGIN_ID.into(),
                capabilities: protocol::METHODS.iter().map(|s| (*s).into()).collect(),
                inherited_workspace,
            }),
        }),
    )?;
    let mut last_id = request.id;
    while let Some(request) = read_request(input)? {
        if request.protocol_version != protocol::VERSION || request.id <= last_id {
            return Err(Error::new("invalid Host V1 version or request sequence"));
        }
        last_id = request.id;
        // Decode the complete operation before any backend call. Duplicate keys
        // were already rejected by the public protocol parser, at every depth.
        let operation =
            serde_json::from_value(json!({"method":request.method, "params":request.params}))?;
        let response = match dispatch(&host, operation) {
            Ok(result) => Response::Success(Success {
                protocol_version: protocol::VERSION,
                id: request.id,
                result,
            }),
            Err(DispatchError(error)) => Response::Failure(Failure {
                protocol_version: protocol::VERSION,
                id: request.id,
                error,
            }),
        };
        write_response(output, response)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ByteReader<'a> {
        bytes: &'a [u8],
        interrupt: bool,
    }

    impl Read for ByteReader<'_> {
        fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
            if std::mem::take(&mut self.interrupt) {
                return Err(std::io::ErrorKind::Interrupted.into());
            }
            let length = output.len().min(1);
            self.bytes.read(&mut output[..length])
        }
    }

    #[derive(Default)]
    struct ByteWriter {
        bytes: Vec<u8>,
        flushed: bool,
    }

    impl Write for ByteWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let length = bytes.len().min(1);
            self.bytes.extend_from_slice(&bytes[..length]);
            Ok(length)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.flushed = true;
            Ok(())
        }
    }

    #[test]
    fn fragmented_io_handles_interrupts_short_writes_and_flushes() {
        let body = br#"{"protocol_version":1,"id":2,"method":"workspaces","params":{}}"#;
        let frame = [&(body.len() as u32).to_be_bytes()[..], body].concat();
        let mut reader = ByteReader {
            bytes: &frame,
            interrupt: true,
        };
        let request = read_request(&mut reader).unwrap().unwrap();
        assert_eq!(request.id, 2);
        assert!(read_request(&mut reader).unwrap().is_none());
        let mut output = ByteWriter::default();
        write_response(
            &mut output,
            Response::Success(Success {
                protocol_version: 1,
                id: request.id,
                result: Value::Null,
            }),
        )
        .unwrap();
        assert!(output.flushed);
        assert_eq!(
            u32::from_be_bytes(output.bytes[..4].try_into().unwrap()) as usize,
            output.bytes.len() - 4
        );
        let response: Response = protocol::from_slice(&output.bytes[4..]).unwrap();
        assert!(
            matches!(response, Response::Success(reply) if reply.id == 2 && reply.result.is_null())
        );
    }

    #[test]
    fn exact_frame_limit_is_accepted_and_trailing_json_is_rejected() {
        let mut body =
            br#"{"protocol_version":1,"id":2,"method":"workspaces","params":{}}"#.to_vec();
        body.resize(protocol::MAX_FRAME_BYTES, b' ');
        let frame = [&(body.len() as u32).to_be_bytes()[..], &body].concat();
        assert!(read_request(&mut frame.as_slice()).unwrap().is_some());
        let length = body.len();
        body[length - 2..].copy_from_slice(b"{}");
        let frame = [&(body.len() as u32).to_be_bytes()[..], &body].concat();
        assert!(read_request(&mut frame.as_slice()).is_err());
    }
}
