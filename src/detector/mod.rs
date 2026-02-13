pub mod patterns;

use std::path::Path;

/// Detect all project types present in a directory
pub fn detect_project_types(root: &Path) -> Vec<String> {
    let mut types = Vec::new();

    for (name, check_fn) in patterns::all_patterns() {
        if check_fn(root) {
            types.push(name.to_string());
        }
    }

    types
}
