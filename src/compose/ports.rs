use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use std::net::TcpListener;

use crate::config::lock::FileLock;
use crate::error::{GrootError, Result};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AllocatedPorts {
    pub app: u16,
    pub db: u16,
    pub redis: u16,
}

#[derive(Serialize, Deserialize, Debug, Default)]
struct PortRegistry {
    allocations: HashMap<String, AllocatedPorts>,
}

const APP_BASE: u16 = 3001;
const DB_BASE: u16 = 5433;
const REDIS_BASE: u16 = 6380;

/// Allocate ports for a worker, using gap-filling to reuse freed slots.
pub fn allocate(groot_dir: &Path, worker_name: &str) -> Result<AllocatedPorts> {
    let registry_path = groot_dir.join("ports.json");
    let lock_path = groot_dir.join("ports.json.lock");
    let _lock = FileLock::acquire(&lock_path)?;

    let mut registry = load_registry(&registry_path);

    // If already allocated, return existing
    if let Some(existing) = registry.allocations.get(worker_name) {
        return Ok(existing.clone());
    }

    // Find lowest available index (gap-filling)
    let used_indices: Vec<u16> = registry
        .allocations
        .values()
        .map(|p| p.app - APP_BASE)
        .collect();

    let mut index: u16 = 0;
    while used_indices.contains(&index) {
        index += 1;
    }

    let ports = AllocatedPorts {
        app: APP_BASE + index,
        db: DB_BASE + index,
        redis: REDIS_BASE + index,
    };

    registry
        .allocations
        .insert(worker_name.to_string(), ports.clone());
    save_registry(&registry_path, &registry)?;

    Ok(ports)
}

/// Release ports for a worker.
pub fn release(groot_dir: &Path, worker_name: &str) -> Result<()> {
    let registry_path = groot_dir.join("ports.json");
    let lock_path = groot_dir.join("ports.json.lock");
    let _lock = FileLock::acquire(&lock_path)?;

    let mut registry = load_registry(&registry_path);
    registry.allocations.remove(worker_name);
    save_registry(&registry_path, &registry)?;

    Ok(())
}

/// Check that all allocated ports are available before starting compose.
pub fn check_ports_available(ports: &AllocatedPorts) -> Result<()> {
    let checks = [
        (ports.app, "app"),
        (ports.db, "db"),
        (ports.redis, "redis"),
    ];

    for (port, service) in checks {
        if TcpListener::bind(("0.0.0.0", port)).is_err() {
            return Err(GrootError::PortInUse {
                port,
                service: service.to_string(),
            });
        }
    }

    Ok(())
}

fn load_registry(path: &Path) -> PortRegistry {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_registry(path: &Path, registry: &PortRegistry) -> Result<()> {
    let contents = serde_json::to_string_pretty(registry)?;
    std::fs::write(path, contents)?;
    Ok(())
}
