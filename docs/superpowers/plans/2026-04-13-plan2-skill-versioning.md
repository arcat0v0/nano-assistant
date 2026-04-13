# Skill Versioning & Builtin Protection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Embed system skills into the binary at compile time, synchronize their version with the nano-assistant version, and prevent user skills from overriding them.

**Architecture:** System skills are loaded via `include_str!()` before any filesystem skills. A `SkillSource` enum and `is_builtin` flag on `Skill` track provenance. The dedup logic in `load_skills_multi` is extended to reject user skills that collide with builtin names.

**Tech Stack:** Rust, `include_str!()`, `env!("CARGO_PKG_VERSION")`

**Depends on:** Plan 1 (Platform Abstraction) — uses `Platform::skills_dir()`

---

### Task 1: Add SkillSource enum and is_builtin field to Skill

**Files:**
- Modify: `src/skills/mod.rs`

- [ ] **Step 1: Add SkillSource enum**

After the existing `SkillMarkdownMeta` struct (around line 82), add:

```rust
/// Where a skill was loaded from.
#[derive(Debug, Clone, PartialEq)]
pub enum SkillSource {
    /// Compiled into the binary.
    Builtin,
    /// User's skill directory.
    UserDir(PathBuf),
    /// skills.sh ecosystem (~/.agents/skills/).
    SkillsSh,
    /// Extra path from config.
    ExtraPath(PathBuf),
}

impl std::fmt::Display for SkillSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SkillSource::Builtin => write!(f, "builtin"),
            SkillSource::UserDir(p) => write!(f, "{}", p.display()),
            SkillSource::SkillsSh => write!(f, "~/.agents/skills/"),
            SkillSource::ExtraPath(p) => write!(f, "{}", p.display()),
        }
    }
}
```

- [ ] **Step 2: Add fields to Skill struct**

Add two new fields to the `Skill` struct. Mark them `#[serde(skip)]` since they're runtime-only:

```rust
#[serde(skip)]
pub is_builtin: bool,
#[serde(skip)]
pub source: Option<SkillSource>,
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check 2>&1 | head -20`
Expected: no errors. Existing code initializes Skill via struct literal or deserialization — `#[serde(skip)]` gives `false`/`None` defaults.

- [ ] **Step 4: Commit**

```bash
git add src/skills/mod.rs
git commit -m "feat(skills): add SkillSource enum and is_builtin field"
```

---

### Task 2: Embed builtin skills with include_str!()

**Files:**
- Modify: `src/skills/mod.rs`

- [ ] **Step 1: Add builtin skill definitions**

Near the top of `src/skills/mod.rs` (after the constants), add:

```rust
const NA_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Builtin skills compiled into the binary.
/// Each entry: (directory_name, SKILL.md content).
const BUILTIN_SKILLS: &[(&str, &str)] = &[
    ("database-admin", include_str!("../../skills/database-admin/SKILL.md")),
    ("server-security", include_str!("../../skills/server-security/SKILL.md")),
    ("container-orchestration", include_str!("../../skills/container-orchestration/SKILL.md")),
];
```

- [ ] **Step 2: Add load_builtin_skills function**

```rust
/// Load all builtin skills from compiled-in SKILL.md content.
/// Version is forced to match the binary version.
fn load_builtin_skills() -> Vec<Skill> {
    let mut skills = Vec::new();
    for (name, content) in BUILTIN_SKILLS {
        let parsed = parse_skill_markdown(content);
        let mut skill = Skill {
            name: parsed.meta.name.unwrap_or_else(|| name.to_string()),
            description: parsed
                .meta
                .description
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| extract_description(&parsed.body)),
            version: NA_VERSION.to_string(),
            author: parsed.meta.author,
            tags: parsed.meta.tags,
            tools: Vec::new(),
            prompts: if parsed.body.trim().is_empty() {
                Vec::new()
            } else {
                vec![parsed.body]
            },
            location: None,
            is_builtin: true,
            source: Some(SkillSource::Builtin),
        };
        // Parse tools from markdown if present
        skill.tools = extract_tools_from_markdown(content);
        skills.push(skill);
    }
    skills
}
```

Note: `extract_tools_from_markdown` may or may not exist. Check if `load_skill_md` has inline tool extraction logic. If builtin skills don't define callable tools (current database-admin/server-security/container-orchestration are instruction-only), the tools vec stays empty and this is fine.

- [ ] **Step 3: Verify it compiles**

Run: `cargo check 2>&1 | head -20`
Expected: compiles. The `skills/` directory files must exist at compile time.

- [ ] **Step 4: Commit**

```bash
git add src/skills/mod.rs
git commit -m "feat(skills): embed builtin skills via include_str with version sync"
```

---

### Task 3: Integrate builtins into load_skills with protection

**Files:**
- Modify: `src/skills/mod.rs`

- [ ] **Step 1: Modify load_skills to prepend builtins**

Replace the body of `load_skills()`:

```rust
pub fn load_skills(config: &crate::config::SkillsConfig) -> Vec<Skill> {
    // 1. Load builtins first (highest priority)
    let builtins = load_builtin_skills();
    let builtin_names: HashSet<String> = builtins.iter().map(|s| s.name.clone()).collect();

    // 2. Resolve filesystem directories
    let primary = match &config.skills_dir {
        Some(dir) => PathBuf::from(dir),
        None => skills_dir(),
    };
    let agents_skills = agents_skills_dir();

    let mut dirs: Vec<PathBuf> = vec![primary];
    if agents_skills.exists() {
        dirs.push(agents_skills);
    }
    for extra in &config.extra_paths {
        let expanded = expand_tilde(extra);
        dirs.push(expanded);
    }

    // 3. Load filesystem skills, rejecting builtin name collisions
    let dir_refs: Vec<&Path> = dirs.iter().map(|p| p.as_path()).collect();
    let fs_skills = load_skills_multi(&dir_refs, config.allow_scripts);

    let mut all_skills = builtins;
    for skill in fs_skills {
        if builtin_names.contains(&skill.name) {
            eprintln!(
                "warning: skill '{}' is builtin, user version from {} ignored",
                skill.name,
                skill.source.as_ref().map(|s| s.to_string()).unwrap_or_default()
            );
            tracing::warn!(
                "skipping user skill '{}': name conflicts with builtin skill",
                skill.name
            );
        } else {
            all_skills.push(skill);
        }
    }

    all_skills
}
```

- [ ] **Step 2: Add source tracking to load_skills_from_directory**

Modify `load_skills_from_directory` to accept and set a `SkillSource` on each loaded skill. Add a parameter:

```rust
pub fn load_skills_from_directory(
    skills_dir: &Path,
    allow_scripts: bool,
    source: SkillSource,
) -> Vec<Skill> {
```

At the end of each successful skill load (after `skills.push(skill)`), set the source:

```rust
if let Some(last) = skills.last_mut() {
    last.source = Some(source.clone());
}
```

- [ ] **Step 3: Update load_skills_multi to pass source info**

Modify `load_skills_multi` to determine source for each directory and pass it through. Replace the function:

```rust
pub fn load_skills_multi(dirs: &[&Path], allow_scripts: bool) -> Vec<Skill> {
    let mut seen_names: HashSet<String> = HashSet::new();
    let mut all_skills: Vec<Skill> = Vec::new();

    let agents_dir = agents_skills_dir();

    for dir in dirs {
        let source = if *dir == agents_dir.as_path() {
            SkillSource::SkillsSh
        } else {
            SkillSource::UserDir(dir.to_path_buf())
        };

        let dir_skills = load_skills_from_directory(dir, allow_scripts, source);
        for skill in dir_skills {
            if seen_names.insert(skill.name.clone()) {
                all_skills.push(skill);
            } else {
                tracing::debug!(
                    "skipping duplicate skill '{}' from {}",
                    skill.name,
                    dir.display()
                );
            }
        }
    }

    all_skills
}
```

- [ ] **Step 4: Verify it compiles and tests pass**

Run: `cargo check && cargo test 2>&1 | tail -10`
Expected: compiles, tests pass

- [ ] **Step 5: Commit**

```bash
git add src/skills/mod.rs
git commit -m "feat(skills): builtin protection — reject user skills with builtin names"
```

---

### Task 4: Update CLI skills list to show source

**Files:**
- Modify: `src/cli/commands.rs`

- [ ] **Step 1: Update handle_skills_command List branch**

Find the `SkillsSubcommand::List` handler and update to include SOURCE column:

```rust
SkillsSubcommand::List => {
    let config = load_config(&crate::config::default_config_path());
    let skills = crate::skills::load_skills(&config.skills);

    if skills.is_empty() {
        println!("No skills installed.");
    } else {
        println!("{:<30} {:<10} {}", "NAME", "VERSION", "SOURCE");
        for skill in &skills {
            let source = skill
                .source
                .as_ref()
                .map(|s| s.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            println!("{:<30} {:<10} {}", skill.name, skill.version, source);
        }
    }
}
```

- [ ] **Step 2: Verify with cargo run**

Run: `cargo run -- skills list 2>&1`
Expected: shows builtin skills with version matching Cargo.toml and "builtin" source

- [ ] **Step 3: Commit**

```bash
git add src/cli/commands.rs
git commit -m "feat(cli): show skill source in skills list output"
```

---

### Task 5: Verify end-to-end

**Files:** None (verification only)

- [ ] **Step 1: Full build**

Run: `cargo build 2>&1 | tail -5`

- [ ] **Step 2: Test skills list**

Run: `cargo run -- skills list`
Expected output should show builtin skills with matching version and "builtin" source.

- [ ] **Step 3: Test builtin protection**

Create a dummy skill with a builtin name to verify the warning:

```bash
mkdir -p /tmp/test-skills/database-admin
cat > /tmp/test-skills/database-admin/SKILL.md << 'EOF'
---
name: database-admin
description: fake override
version: 99.0.0
---
# Fake
EOF
```

Run na with extra path pointing to it and verify the warning is printed.

- [ ] **Step 4: Clean up test files**

```bash
rm -rf /tmp/test-skills
```
