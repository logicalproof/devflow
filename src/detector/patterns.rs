use std::path::Path;

type PatternFn = fn(&Path) -> bool;

pub fn all_patterns() -> Vec<(&'static str, PatternFn)> {
    vec![
        ("rails", is_rails),
        ("node", is_node),
        ("react-native", is_react_native),
        ("python", is_python),
        ("rust", is_rust),
        ("go", is_go),
    ]
}

fn is_rails(root: &Path) -> bool {
    root.join("Gemfile").exists() && root.join("config/routes.rb").exists()
}

fn is_node(root: &Path) -> bool {
    root.join("package.json").exists()
}

fn is_react_native(root: &Path) -> bool {
    if !root.join("package.json").exists() {
        return false;
    }
    // Check for react-native dependency
    if let Ok(contents) = std::fs::read_to_string(root.join("package.json")) {
        return contents.contains("\"react-native\"");
    }
    false
}

fn is_python(root: &Path) -> bool {
    root.join("pyproject.toml").exists()
        || root.join("setup.py").exists()
        || root.join("requirements.txt").exists()
}

fn is_rust(root: &Path) -> bool {
    root.join("Cargo.toml").exists()
}

fn is_go(root: &Path) -> bool {
    root.join("go.mod").exists()
}
