//! Docker Engine API JSON → domain types. Pure functions, tested on their own.

use kai_domain::{
    Container, ContainerId, ContainerState, EngineEvent, EngineInfo, PortMapping, Protocol,
    RuntimeError,
};
use serde_json::Value;

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_owned()
}

/// One entry of `GET /containers/json`.
pub fn container(v: &Value) -> Result<Container, RuntimeError> {
    let id = ContainerId::new(text(&v["Id"]))
        .map_err(|e| RuntimeError::Engine(format!("engine listed a container with {e}")))?;
    let name = v["Names"]
        .as_array()
        .and_then(|n| n.first())
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim_start_matches('/')
        .to_owned();
    let mut ports: Vec<PortMapping> = Vec::new();
    for p in v["Ports"].as_array().into_iter().flatten() {
        let Some(container_port) = p["PrivatePort"]
            .as_u64()
            .and_then(|n| u16::try_from(n).ok())
        else {
            continue;
        };
        let protocol =
            Protocol::parse(p["Type"].as_str().unwrap_or("tcp")).unwrap_or(Protocol::Tcp);
        let host_port = p["PublicPort"]
            .as_u64()
            .and_then(|n| u16::try_from(n).ok())
            .filter(|&n| n != 0);
        let host_ip = p["IP"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        // Docker lists a binding once per address family (0.0.0.0 and ::).
        if !ports.iter().any(|q| {
            q.container_port == container_port && q.host_port == host_port && q.protocol == protocol
        }) {
            ports.push(PortMapping {
                container_port,
                host_port,
                host_ip,
                protocol,
            });
        }
    }
    ports.sort_by_key(|p| (p.container_port, p.host_port));
    Ok(Container {
        name: if name.is_empty() {
            id.short().to_owned()
        } else {
            name
        },
        image: text(&v["Image"]),
        state: ContainerState::parse(v["State"].as_str().unwrap_or("")),
        status: text(&v["Status"]),
        ports,
        created_unix: v["Created"].as_i64().unwrap_or(0),
        compose_project: v["Labels"]["com.docker.compose.project"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
        id,
    })
}

/// `GET /containers/json`'s whole answer.
pub fn containers(v: &Value) -> Result<Vec<Container>, RuntimeError> {
    v.as_array()
        .ok_or_else(|| RuntimeError::Engine("container list is not a JSON array".into()))?
        .iter()
        .map(container)
        .collect()
}

/// `GET /version`. Kiri reports its own version as `KiriVersion`; older Kiri
/// engines put it in `Version`.
pub fn engine_info(v: &Value) -> Result<EngineInfo, RuntimeError> {
    if !v.is_object() {
        return Err(RuntimeError::Engine(
            "version answer is not a JSON object".into(),
        ));
    }
    let engine_version = v["KiriVersion"]
        .as_str()
        .or_else(|| v["Version"].as_str())
        .unwrap_or("unknown")
        .to_owned();
    Ok(EngineInfo {
        engine_version,
        api_version: text(&v["ApiVersion"]),
        os: text(&v["Os"]),
        arch: text(&v["Arch"]),
    })
}

/// One line of `GET /events`. `None` for lines that are not events.
pub fn event(line: &[u8]) -> Option<EngineEvent> {
    let v: Value = serde_json::from_slice(line).ok()?;
    let kind = v["Type"].as_str()?;
    let action = v["Action"].as_str().unwrap_or_default().to_owned();
    if kind == "container" {
        // An event naming no valid container is still a change worth a refresh.
        match ContainerId::new(text(&v["Actor"]["ID"])) {
            Ok(id) => Some(EngineEvent::ContainerChanged { id, action }),
            Err(_) => Some(EngineEvent::Other {
                kind: kind.to_owned(),
                action,
            }),
        }
    } else {
        Some(EngineEvent::Other {
            kind: kind.to_owned(),
            action,
        })
    }
}

/// An HTTP status (and the engine's message) as a port error.
pub fn status_error(status: u16, message: String) -> RuntimeError {
    match status {
        404 => RuntimeError::NotFound(message),
        409 => RuntimeError::Conflict(message),
        400..=499 => RuntimeError::Rejected(message),
        _ => RuntimeError::Engine(if message.is_empty() {
            format!("engine answered HTTP {status}")
        } else {
            message
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ID: &str = "4f1c0a5e2b3d4c5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4c5d6";

    #[test]
    fn maps_a_listed_container() {
        let v = json!({
            "Id": ID, "Names": ["/web-1"], "Image": "nginx:1.27", "State": "running", "Status": "Up 5 minutes",
            "Created": 1_700_000_000,
            "Ports": [
                {"IP": "0.0.0.0", "PrivatePort": 80, "PublicPort": 8080, "Type": "tcp"},
                {"IP": "::", "PrivatePort": 80, "PublicPort": 8080, "Type": "tcp"},
                {"PrivatePort": 53, "Type": "udp"}
            ],
            "Labels": {"com.docker.compose.project": "shop"}
        });
        let c = container(&v).unwrap();
        assert_eq!(c.id.as_str(), ID);
        assert_eq!(c.name, "web-1");
        assert_eq!(c.state, ContainerState::Running);
        assert_eq!(
            c.ports.iter().map(ToString::to_string).collect::<Vec<_>>(),
            ["53/udp", "8080→80/tcp"]
        );
        assert_eq!(c.compose_project.as_deref(), Some("shop"));
        assert_eq!(c.created_unix, 1_700_000_000);
    }

    #[test]
    fn a_container_without_a_name_is_shown_by_its_short_id() {
        let c = container(&json!({"Id": ID, "State": "exited"})).unwrap();
        assert_eq!(c.name, &ID[..12]);
        assert!(c.ports.is_empty());
    }

    #[test]
    fn an_id_that_could_alter_a_request_path_is_refused() {
        assert!(matches!(
            container(&json!({"Id": "../../etc", "State": "running"})),
            Err(RuntimeError::Engine(_))
        ));
    }

    #[test]
    fn engine_info_prefers_kiris_own_version() {
        let i = engine_info(&json!({"Version": "28.0.0", "KiriVersion": "0.2.3", "ApiVersion": "1.47", "Os": "linux", "Arch": "arm64"})).unwrap();
        assert_eq!(
            (i.engine_version.as_str(), i.api_version.as_str()),
            ("0.2.3", "1.47")
        );
        assert_eq!(
            engine_info(&json!({"Version": "0.1.9"}))
                .unwrap()
                .engine_version,
            "0.1.9"
        );
        assert!(engine_info(&json!("nope")).is_err());
    }

    #[test]
    fn events_map_containers_and_keep_other_kinds() {
        let line = format!(
            r#"{{"Type":"container","Action":"die","Actor":{{"ID":"{ID}","Attributes":{{}}}}}}"#
        );
        assert_eq!(
            event(line.as_bytes()),
            Some(EngineEvent::ContainerChanged {
                id: ContainerId::new(ID).unwrap(),
                action: "die".into()
            })
        );
        assert_eq!(
            event(br#"{"Type":"image","Action":"pull","Actor":{"ID":"alpine"}}"#),
            Some(EngineEvent::Other {
                kind: "image".into(),
                action: "pull".into()
            })
        );
        assert_eq!(event(b"not json"), None);
        assert_eq!(event(b"{}"), None);
    }

    #[test]
    fn statuses_become_port_errors() {
        assert_eq!(
            status_error(404, "No such container: x".into()),
            RuntimeError::NotFound("No such container: x".into())
        );
        assert!(matches!(
            status_error(409, String::new()),
            RuntimeError::Conflict(_)
        ));
        assert!(matches!(
            status_error(400, String::new()),
            RuntimeError::Rejected(_)
        ));
        assert_eq!(
            status_error(500, String::new()),
            RuntimeError::Engine("engine answered HTTP 500".into())
        );
    }
}
