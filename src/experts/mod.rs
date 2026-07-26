use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpertProfile {
    pub name: String,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub match_files: Vec<String>,
}

fn experts_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".config")
        .join("ai-code")
        .join("experts")
}

pub fn ensure_defaults() {
    let dir = experts_dir();
    let _ = std::fs::create_dir_all(&dir);

    let defaults: &[(&str, ExpertProfile)] = &[
        (
            "nextjs.json",
            ExpertProfile {
                name: "Next.js Frontend Expert".into(),
                prompt: "You are an expert frontend developer specializing in Next.js 15 with the App Router. Follow these conventions strictly:\n\n- Use React Server Components by default. Add 'use client' only when absolutely necessary (event handlers, state, effects).\n- Use Tailwind CSS for all styling. Prefer utility classes over custom CSS.\n- Use TypeScript with strict mode. Always define proper interfaces and types.\n- Use path alias @/ instead of relative imports.\n- For forms, use server actions in app/actions/. Validate with Zod.\n- For data fetching, use fetch in server components or React Query on the client.\n- Structure: src/app/ (routes), src/components/ (shared UI), src/lib/ (utilities), src/types/ (TypeScript types).\n- Always use proper SEO metadata and generateMetadata.\n- Error boundaries at the route level, loading.tsx for suspense.\n- Prefer Edge Runtime for performance-critical routes.\n- Generate Zod schemas from TypeScript types, not the other way around.\n- Always use absolute paths for file operations within the project.".into(),
                match_files: vec![
                    "next.config.js".into(),
                    "next.config.ts".into(),
                    "next.config.mjs".into(),
                ],
            },
        ),
        (
            "rust.json",
            ExpertProfile {
                name: "Rust Systems Expert".into(),
                prompt: "You are an expert Rust developer. Follow these conventions:\n\n- Use Rust 2024 edition. Prefer idiomatic patterns over unsafe code.\n- Use anyhow for application-level errors, thiserror for library errors.\n- Prefer iterators and combinators over explicit loops.\n- Use serde for serialization, reqwest for HTTP.\n- Use tokio for async runtime. Prefer spawning over blocking.\n- Follow Rust naming conventions (snake_case for functions, CamelCase for types).\n- Use clippy and always fix warnings.\n- Prefer Result<_, _> over panicking. Use .context() from anyhow.\n- Write comprehensive doc comments (///) for public APIs.\n- Use &str for function parameters unless you need ownership.\n- Prefer Arc<str> over Arc<String> for immutable shared strings.\n- Always use absolute paths for file operations within the project.".into(),
                match_files: vec!["Cargo.toml".into(), "Cargo.lock".into()],
            },
        ),
        (
            "python.json",
            ExpertProfile {
                name: "Python Expert".into(),
                prompt: "You are an expert Python developer. Follow these conventions:\n\n- Use Python 3.12+. Use type hints everywhere.\n- Use Pydantic for data validation and settings management.\n- Use FastAPI for web APIs, pytest for testing.\n- Use uv or poetry for dependency management (prefer uv).\n- Follow PEP 8 style conventions. Use black for formatting.\n- Use pathlib.Path instead of os.path.\n- Prefer dataclasses or Pydantic models over plain dicts.\n- Use async/await with proper async libraries (httpx, aiofiles).\n- Use ruff for linting, mypy for type checking.\n- Use logging module instead of print statements.\n- Write docstrings for all public functions and classes.\n- Use context managers (with statements) for resource management.\n- Always use absolute paths for file operations within the project.".into(),
                match_files: vec![
                    "requirements.txt".into(),
                    "pyproject.toml".into(),
                    "setup.py".into(),
                ],
            },
        ),
    ];

    for (filename, profile) in defaults {
        let path = dir.join(filename);
        if !path.exists() {
            if let Ok(json) = serde_json::to_string_pretty(profile) {
                let _ = std::fs::write(&path, json);
            }
        }
    }
}

pub fn list_profiles() -> Vec<(String, ExpertProfile)> {
    ensure_defaults();
    let dir = experts_dir();
    let mut profiles = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    if let Ok(data) = std::fs::read_to_string(&path) {
                        if let Ok(profile) = serde_json::from_str::<ExpertProfile>(&data) {
                            profiles.push((stem.to_string(), profile));
                        }
                    }
                }
            }
        }
    }
    profiles.sort_by(|a, b| a.1.name.cmp(&b.1.name));
    profiles
}

pub fn load_profile(slug: &str) -> Option<ExpertProfile> {
    ensure_defaults();
    let path = experts_dir().join(format!("{}.json", slug));
    if path.exists() {
        if let Ok(data) = std::fs::read_to_string(&path) {
            if let Ok(profile) = serde_json::from_str(&data) {
                return Some(profile);
            }
        }
    }
    None
}

pub fn detect_project(cwd: &Path) -> Option<(String, ExpertProfile)> {
    for (slug, profile) in list_profiles() {
        for file_pattern in &profile.match_files {
            if cwd.join(file_pattern).exists() {
                return Some((slug, profile));
            }
        }
    }
    None
}
