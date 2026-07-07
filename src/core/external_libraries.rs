use std::{
    collections::{BTreeSet, VecDeque},
    fs,
    path::{Path, PathBuf},
};

use toml::Value;

const MAX_EXTERNAL_CRATES: usize = 180;
const MAX_EXTERNAL_GO_MODULES: usize = 180;
const MAX_EXTERNAL_FILES: usize = 8000;
const MAX_FILES_PER_CRATE: usize = 220;
const MAX_FILES_PER_GO_MODULE: usize = 220;
const MAX_SOURCE_FILE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, Default)]
pub struct ExternalLibraryDiscovery {
    pub direct_dependencies: Vec<CargoPackage>,
    pub lock_packages: Vec<CargoPackage>,
    pub files: Vec<PathBuf>,
}

#[derive(Debug, Clone, Default)]
pub struct GoExternalLibraryDiscovery {
    pub modules: Vec<GoModule>,
    pub files: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CargoPackage {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct GoModule {
    pub path: String,
    pub version: String,
}

pub fn discover_cargo_external_libraries(roots: &[PathBuf]) -> ExternalLibraryDiscovery {
    let mut direct_dependencies = BTreeSet::new();
    let mut lock_packages = Vec::new();
    let mut workspace_packages = BTreeSet::new();

    for root in roots {
        let manifest_path = root.join("Cargo.toml");
        if manifest_path.is_file()
            && let Some(manifest) = read_toml(&manifest_path)
        {
            workspace_packages.extend(parse_manifest_package(&manifest));
            direct_dependencies.extend(parse_manifest_dependencies(&manifest));
        }

        let lock_path = root.join("Cargo.lock");
        if lock_path.is_file()
            && let Some(lock) = read_toml(&lock_path)
        {
            lock_packages.extend(parse_lock_package_entries(&lock));
        }
    }

    let direct_dependencies = direct_dependencies.into_iter().collect::<Vec<_>>();
    let lock_packages = prioritize_lock_packages(
        lock_packages,
        &direct_dependencies,
        &workspace_packages.into_iter().collect::<Vec<_>>(),
    )
    .into_iter()
    .take(MAX_EXTERNAL_CRATES)
    .collect::<Vec<_>>();

    let files = discover_package_files(&lock_packages);

    ExternalLibraryDiscovery {
        direct_dependencies,
        lock_packages,
        files,
    }
}

pub fn discover_go_external_libraries(roots: &[PathBuf]) -> GoExternalLibraryDiscovery {
    let mut modules = BTreeSet::new();

    for root in roots {
        let go_mod_path = root.join("go.mod");
        if go_mod_path.is_file()
            && let Ok(text) = fs::read_to_string(&go_mod_path)
        {
            modules.extend(parse_go_mod_requires(&text));
        }
    }

    let modules = modules
        .into_iter()
        .take(MAX_EXTERNAL_GO_MODULES)
        .collect::<Vec<_>>();
    let files = discover_go_module_files(&modules);

    GoExternalLibraryDiscovery { modules, files }
}

fn read_toml(path: &Path) -> Option<Value> {
    toml::from_str(&fs::read_to_string(path).ok()?).ok()
}

fn parse_manifest_package(manifest: &Value) -> Option<CargoPackage> {
    let package = manifest.get("package")?;
    Some(CargoPackage {
        name: package.get("name")?.as_str()?.to_owned(),
        version: package.get("version")?.as_str()?.to_owned(),
    })
}

fn parse_manifest_dependencies(manifest: &Value) -> Vec<CargoPackage> {
    let mut dependencies = BTreeSet::new();

    for table_name in ["dependencies", "dev-dependencies", "build-dependencies"] {
        collect_dependency_table(manifest.get(table_name), &mut dependencies);
    }

    if let Some(targets) = manifest.get("target").and_then(Value::as_table) {
        for target in targets.values() {
            for table_name in ["dependencies", "dev-dependencies", "build-dependencies"] {
                collect_dependency_table(target.get(table_name), &mut dependencies);
            }
        }
    }

    dependencies.into_iter().collect()
}

fn collect_dependency_table(value: Option<&Value>, dependencies: &mut BTreeSet<CargoPackage>) {
    let Some(table) = value.and_then(Value::as_table) else {
        return;
    };

    for (name, value) in table {
        let package_name = value
            .as_table()
            .and_then(|table| table.get("package"))
            .and_then(Value::as_str)
            .unwrap_or(name);
        let version = match value {
            Value::String(version) => version.as_str(),
            Value::Table(table) => table.get("version").and_then(Value::as_str).unwrap_or("*"),
            _ => "*",
        };

        dependencies.insert(CargoPackage {
            name: package_name.to_owned(),
            version: version.to_owned(),
        });
    }
}

#[derive(Debug, Clone)]
struct LockPackage {
    package: CargoPackage,
    dependencies: Vec<String>,
}

fn parse_lock_package_entries(lock: &Value) -> Vec<LockPackage> {
    lock.get("package")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|package| {
            Some(LockPackage {
                package: CargoPackage {
                    name: package.get("name")?.as_str()?.to_owned(),
                    version: package.get("version")?.as_str()?.to_owned(),
                },
                dependencies: package
                    .get("dependencies")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(lock_dependency_name)
                    .collect(),
            })
        })
        .collect()
}

fn prioritize_lock_packages(
    lock_packages: Vec<LockPackage>,
    direct_dependencies: &[CargoPackage],
    workspace_packages: &[CargoPackage],
) -> Vec<CargoPackage> {
    let mut prioritized_names = BTreeSet::new();
    let mut queue = direct_dependencies
        .iter()
        .map(|dependency| dependency.name.clone())
        .collect::<VecDeque<_>>();

    while let Some(name) = queue.pop_front() {
        if !prioritized_names.insert(name.clone()) {
            continue;
        }

        for package in lock_packages
            .iter()
            .filter(|package| package.package.name == name)
        {
            for dependency in &package.dependencies {
                if !prioritized_names.contains(dependency) {
                    queue.push_back(dependency.clone());
                }
            }
        }
    }

    let workspace_packages = workspace_packages.iter().collect::<BTreeSet<_>>();
    let mut result = Vec::new();

    for name in &prioritized_names {
        for package in lock_packages
            .iter()
            .filter(|package| package.package.name == *name)
        {
            if !workspace_packages.contains(&package.package) && !result.contains(&package.package)
            {
                result.push(package.package.clone());
            }
        }
    }

    let mut remaining = lock_packages
        .into_iter()
        .map(|package| package.package)
        .filter(|package| !workspace_packages.contains(package))
        .collect::<Vec<_>>();
    remaining.sort();

    for package in remaining {
        if !result.contains(&package) {
            result.push(package);
        }
    }

    result
}

fn lock_dependency_name(dependency: &str) -> String {
    dependency
        .split_whitespace()
        .next()
        .unwrap_or(dependency)
        .to_owned()
}

fn discover_package_files(packages: &[CargoPackage]) -> Vec<PathBuf> {
    let Some(cargo_home) = cargo_home() else {
        return Vec::new();
    };

    let registry_roots = registry_source_roots(&cargo_home);
    let git_roots = git_checkout_roots(&cargo_home);
    let mut files = Vec::new();

    for package in packages {
        if files.len() >= MAX_EXTERNAL_FILES {
            break;
        }

        let package_roots = find_package_roots(package, &registry_roots, &git_roots);
        for package_root in package_roots {
            collect_rust_files(&package_root, &mut files, MAX_FILES_PER_CRATE);
            if files.len() >= MAX_EXTERNAL_FILES {
                break;
            }
        }
    }

    files.sort();
    files.dedup();
    files.truncate(MAX_EXTERNAL_FILES);
    files
}

fn cargo_home() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("CARGO_HOME") {
        return Some(PathBuf::from(path));
    }

    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .map(|home| home.join(".cargo"))
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(".cargo"))
        })
}

fn registry_source_roots(cargo_home: &Path) -> Vec<PathBuf> {
    let registry_src = cargo_home.join("registry").join("src");
    let Ok(entries) = fs::read_dir(registry_src) else {
        return Vec::new();
    };

    entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect()
}

fn git_checkout_roots(cargo_home: &Path) -> Vec<PathBuf> {
    let checkouts = cargo_home.join("git").join("checkouts");
    let Ok(entries) = fs::read_dir(checkouts) else {
        return Vec::new();
    };

    entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect()
}

fn find_package_roots(
    package: &CargoPackage,
    registry_roots: &[PathBuf],
    git_roots: &[PathBuf],
) -> Vec<PathBuf> {
    let registry_dir_name = format!("{}-{}", package.name, package.version);
    let mut roots = registry_roots
        .iter()
        .map(|root| root.join(&registry_dir_name))
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();

    roots.extend(find_git_package_roots(package, git_roots));
    roots
}

fn find_git_package_roots(package: &CargoPackage, git_roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut roots = Vec::new();

    for root in git_roots {
        visit_git_checkout(root, package, &mut roots, 0);
    }

    roots
}

fn visit_git_checkout(
    root: &Path,
    package: &CargoPackage,
    package_roots: &mut Vec<PathBuf>,
    depth: usize,
) {
    if depth > 3 || package_roots.len() > 8 {
        return;
    }

    let manifest_path = root.join("Cargo.toml");
    if manifest_path.is_file()
        && let Some(manifest) = read_toml(&manifest_path)
        && parse_manifest_package(&manifest).as_ref() == Some(package)
    {
        package_roots.push(root.to_path_buf());
        return;
    }

    let Ok(entries) = fs::read_dir(root) else {
        return;
    };

    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            visit_git_checkout(&path, package, package_roots, depth + 1);
        }
    }
}

fn collect_rust_files(root: &Path, files: &mut Vec<PathBuf>, max_files: usize) {
    let start_len = files.len();
    visit_rust_files(root, files, start_len + max_files, 0);
}

fn visit_rust_files(root: &Path, files: &mut Vec<PathBuf>, max_total_len: usize, depth: usize) {
    if depth > 16 || files.len() >= max_total_len {
        return;
    }

    let Ok(entries) = fs::read_dir(root) else {
        return;
    };

    for entry in entries.filter_map(Result::ok) {
        if files.len() >= max_total_len {
            return;
        }

        let path = entry.path();
        if path.is_dir() {
            if should_skip_dir(&path) {
                continue;
            }
            visit_rust_files(&path, files, max_total_len, depth + 1);
        } else if is_rust_source_file(&path) {
            files.push(path);
        }
    }
}

fn should_skip_dir(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some(".git" | "target" | "tests" | "benches" | "examples")
    )
}

fn is_rust_source_file(path: &Path) -> bool {
    if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
        return false;
    }

    fs::metadata(path)
        .map(|metadata| metadata.len() <= MAX_SOURCE_FILE_BYTES)
        .unwrap_or(false)
}

fn parse_go_mod_requires(source: &str) -> Vec<GoModule> {
    let mut modules = BTreeSet::new();
    let mut in_require_block = false;

    for line in source.lines() {
        let line = line.split("//").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }

        if line == "require (" {
            in_require_block = true;
            continue;
        }
        if in_require_block && line == ")" {
            in_require_block = false;
            continue;
        }

        if in_require_block {
            if let Some(module) = parse_go_require_fields(line.split_whitespace()) {
                modules.insert(module);
            }
            continue;
        }

        if let Some(rest) = line.strip_prefix("require ") {
            if let Some(module) = parse_go_require_fields(rest.split_whitespace()) {
                modules.insert(module);
            }
        }
    }

    modules.into_iter().collect()
}

fn parse_go_require_fields<'a>(mut fields: impl Iterator<Item = &'a str>) -> Option<GoModule> {
    let path = fields.next()?;
    let version = fields.next()?;

    if path.is_empty() || version.is_empty() {
        return None;
    }

    Some(GoModule {
        path: path.to_owned(),
        version: version.to_owned(),
    })
}

fn discover_go_module_files(modules: &[GoModule]) -> Vec<PathBuf> {
    let Some(go_mod_cache) = go_mod_cache() else {
        return Vec::new();
    };
    let mut files = Vec::new();

    for module in modules {
        if files.len() >= MAX_EXTERNAL_FILES {
            break;
        }

        let module_root = go_mod_cache.join(go_module_cache_dir_name(module));
        if module_root.is_dir() {
            collect_go_files(&module_root, &mut files, MAX_FILES_PER_GO_MODULE);
        }
    }

    files.sort();
    files.dedup();
    files.truncate(MAX_EXTERNAL_FILES);
    files
}

fn go_mod_cache() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("GOMODCACHE") {
        return Some(PathBuf::from(path));
    }

    if let Some(go_path) = std::env::var_os("GOPATH") {
        return Some(PathBuf::from(go_path).join("pkg").join("mod"));
    }

    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .map(|home| home.join("go").join("pkg").join("mod"))
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join("go").join("pkg").join("mod"))
        })
}

fn go_module_cache_dir_name(module: &GoModule) -> PathBuf {
    PathBuf::from(format!(
        "{}@{}",
        escape_go_module_path(&module.path),
        escape_go_module_path(&module.version)
    ))
}

fn escape_go_module_path(value: &str) -> String {
    let mut escaped = String::new();

    for character in value.chars() {
        if character.is_ascii_uppercase() {
            escaped.push('!');
            escaped.push(character.to_ascii_lowercase());
        } else {
            escaped.push(character);
        }
    }

    escaped
}

fn collect_go_files(root: &Path, files: &mut Vec<PathBuf>, max_files: usize) {
    let start_len = files.len();
    visit_go_files(root, files, start_len + max_files, 0);
}

fn visit_go_files(root: &Path, files: &mut Vec<PathBuf>, max_total_len: usize, depth: usize) {
    if depth > 16 || files.len() >= max_total_len {
        return;
    }

    let Ok(entries) = fs::read_dir(root) else {
        return;
    };

    for entry in entries.filter_map(Result::ok) {
        if files.len() >= max_total_len {
            return;
        }

        let path = entry.path();
        if path.is_dir() {
            if should_skip_go_dir(&path) {
                continue;
            }
            visit_go_files(&path, files, max_total_len, depth + 1);
        } else if is_go_source_file(&path) {
            files.push(path);
        }
    }
}

fn should_skip_go_dir(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some(".git" | "vendor" | "testdata")
    )
}

fn is_go_source_file(path: &Path) -> bool {
    if path.extension().and_then(|extension| extension.to_str()) != Some("go") {
        return false;
    }

    fs::metadata(path)
        .map(|metadata| metadata.len() <= MAX_SOURCE_FILE_BYTES)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::{
        GoModule, discover_cargo_external_libraries, escape_go_module_path, parse_go_mod_requires,
    };

    #[test]
    fn discovers_egui_text_edit_source_from_current_workspace_lockfile() {
        let root = std::env::current_dir().expect("current dir");
        let discovery = discover_cargo_external_libraries(&[root]);

        assert!(
            discovery.files.iter().any(|path| {
                path.to_string_lossy()
                    .contains("egui-0.34.2\\src\\widgets\\text_edit\\builder.rs")
                    || path
                        .to_string_lossy()
                        .contains("egui-0.34.2/src/widgets/text_edit/builder.rs")
            }),
            "egui text_edit builder.rs was not discovered"
        );
    }

    #[test]
    fn parses_go_mod_require_directives() {
        let modules = parse_go_mod_requires(
            "module example.com/app\n\nrequire github.com/gin-gonic/gin v1.10.0\n\nrequire (\n    golang.org/x/tools v0.30.0 // indirect\n    github.com/Naoyuki/example v1.2.3\n)\n",
        );

        assert_eq!(
            modules,
            vec![
                GoModule {
                    path: "github.com/Naoyuki/example".to_owned(),
                    version: "v1.2.3".to_owned(),
                },
                GoModule {
                    path: "github.com/gin-gonic/gin".to_owned(),
                    version: "v1.10.0".to_owned(),
                },
                GoModule {
                    path: "golang.org/x/tools".to_owned(),
                    version: "v0.30.0".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn escapes_go_module_cache_path_uppercase_letters() {
        assert_eq!(
            escape_go_module_path("github.com/Naoyuki/MyLib"),
            "github.com/!naoyuki/!my!lib"
        );
    }
}
