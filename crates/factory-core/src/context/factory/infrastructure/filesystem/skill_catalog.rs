use crate::InstalledSkill;
use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};

pub fn list_installed() -> Vec<InstalledSkill> {
    let Some(home) = home_dir() else {
        return Vec::new();
    };

    let mut skills = BTreeMap::new();
    for root in [
        home.join(".agents/skills"),
        home.join(".codex/skills"),
        home.join(".config/opencode/skills"),
        home.join(".opencode/skills"),
        home.join(".claude/skills"),
    ] {
        scan_skill_root(&root, None, &mut skills);
    }
    for cache in [
        home.join(".codex/plugins/cache"),
        home.join(".claude/plugins/cache"),
        home.join(".agents/plugins/cache"),
    ] {
        scan_plugin_cache(&cache, &mut skills);
    }
    if let Some(codex_home) = env::var_os("CODEX_HOME").filter(|value| !value.is_empty()) {
        scan_skill_root(&PathBuf::from(codex_home).join("skills"), None, &mut skills);
    }
    scan_skill_root(Path::new("/etc/codex/skills"), None, &mut skills);

    skills.into_values().collect()
}

pub(crate) fn list_for_workspace(workspace: &Path) -> Vec<InstalledSkill> {
    let mut skills: BTreeMap<_, _> = list_installed()
        .into_iter()
        .map(|skill| (skill.id.clone(), skill))
        .collect();
    for directory in workspace.ancestors() {
        for relative in [
            ".agents/skills",
            ".codex/skills",
            ".opencode/skills",
            ".claude/skills",
        ] {
            scan_skill_root(&directory.join(relative), None, &mut skills);
        }
        if directory.join(".git").exists() {
            break;
        }
    }
    skills.into_values().collect()
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

fn scan_skill_root(
    root: &Path,
    namespace: Option<&str>,
    skills: &mut BTreeMap<String, InstalledSkill>,
) {
    scan_skill_directory(root, namespace, 0, skills);
}

fn scan_skill_directory(
    root: &Path,
    namespace: Option<&str>,
    depth: usize,
    skills: &mut BTreeMap<String, InstalledSkill>,
) {
    if depth > 8 {
        return;
    }
    if root.join("SKILL.md").is_file() {
        add_skill(root, namespace, skills);
        return;
    }

    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let mut directories: Vec<_> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    directories.sort();
    for directory in directories {
        scan_skill_directory(&directory, namespace, depth + 1, skills);
    }
}

fn scan_plugin_cache(root: &Path, skills: &mut BTreeMap<String, InstalledSkill>) {
    visit_plugin_cache(root, root, 0, skills);
}

fn visit_plugin_cache(
    root: &Path,
    current: &Path,
    depth: usize,
    skills: &mut BTreeMap<String, InstalledSkill>,
) {
    if depth > 8 {
        return;
    }
    let Ok(entries) = fs::read_dir(current) else {
        return;
    };
    let mut directories: Vec<_> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    directories.sort();

    for directory in directories {
        if directory.file_name().is_some_and(|name| name == "skills") {
            let relative = directory.strip_prefix(root).unwrap_or(&directory);
            let parts: Vec<_> = relative
                .components()
                .filter_map(|part| part.as_os_str().to_str())
                .collect();
            let namespace = (parts.len() >= 3).then(|| parts[parts.len() - 3]);
            scan_skill_root(&directory, namespace, skills);
        } else {
            visit_plugin_cache(root, &directory, depth + 1, skills);
        }
    }
}

fn add_skill(
    directory: &Path,
    namespace: Option<&str>,
    skills: &mut BTreeMap<String, InstalledSkill>,
) {
    let Ok(contents) = fs::read_to_string(directory.join("SKILL.md")) else {
        return;
    };
    let folder_name = directory
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("skill");
    let name = frontmatter_value(&contents, "name").unwrap_or_else(|| folder_name.to_string());
    let id = match namespace {
        Some(namespace) if !name.contains(':') => format!("{namespace}:{name}"),
        _ => name.clone(),
    };
    // OpenCode v2 keys skill permissions by directory ID, not frontmatter name.
    let opencode_id = match namespace {
        Some(namespace) if !folder_name.contains(':') => format!("{namespace}:{folder_name}"),
        _ => folder_name.to_string(),
    };
    let description = frontmatter_value(&contents, "description").unwrap_or_default();
    skills.entry(id.clone()).or_insert(InstalledSkill {
        id,
        name,
        description,
        opencode_id,
        manifest_path: directory.join("SKILL.md"),
    });
}

fn frontmatter_value(contents: &str, field: &str) -> Option<String> {
    let mut lines = contents.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    for line in lines {
        let line = line.trim_start();
        if line.trim() == "---" {
            break;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key.trim() != field {
            continue;
        }
        let value = value.trim();
        if value.is_empty() || value == ">" || value == "|" {
            return None;
        }
        return Some(
            value
                .trim_matches(|character| character == '\'' || character == '"')
                .to_string(),
        );
    }
    None
}
