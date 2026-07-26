use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamRole {
    pub name: String,
    #[serde(default)]
    pub expert: Option<String>,
    #[serde(default)]
    pub instructions: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamConfig {
    pub name: String,
    #[serde(default)]
    pub prompt: String,
    pub roles: Vec<TeamRole>,
}

fn teams_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".config")
        .join("ai-code")
        .join("teams")
}

pub fn ensure_defaults() {
    let dir = teams_dir();
    let _ = std::fs::create_dir_all(&dir);

    let path = dir.join("se-team.json");
    if path.exists() {
        return;
    }

    let team = TeamConfig {
        name: "Software Engineering Team".into(),
        prompt: "You are leading a software engineering team. You have sub-agents for different roles:\n\n- plan — Project planner for research and design\n- read_code — Codebase architect for understanding existing code\n- backend — Backend developer for server-side implementation\n- frontend — Frontend developer for UI implementation\n\nWorkflow:\n1. First delegate to plan: to research and create a plan\n2. Then delegate to read_code: to understand the relevant codebase areas\n3. Based on findings, delegate to backend: or frontend: for implementation\n4. Review and present results to the user\n\nTo delegate, call the subagent tool with a task string starting with the role name (e.g., \"plan: design the project structure\"). Each sub-agent is an expert in its domain.".into(),
        roles: vec![
            TeamRole {
                name: "plan".into(),
                expert: None,
                instructions: "You are the project planner. Your job is to research, plan, and design. You have access to read, glob, grep, fetch, and bash tools. Use fetch to look up documentation and APIs. Use read/glob/grep to explore the codebase. Produce a detailed implementation plan. Do NOT write any code or modify any files — only produce plans and documentation.".into(),
            },
            TeamRole {
                name: "read_code".into(),
                expert: Some("rust".into()),
                instructions: "You are the codebase architect. Your job is to understand the existing code structure, identify patterns, read relevant source files, and explain how the code works. Use read, glob, and grep to explore the codebase thoroughly. Report your findings to the team lead.".into(),
            },
            TeamRole {
                name: "backend".into(),
                expert: Some("rust".into()),
                instructions: "You are the backend developer. Implement backend features, APIs, data models, and server-side logic. Read relevant files first, then implement changes.".into(),
            },
            TeamRole {
                name: "frontend".into(),
                expert: Some("nextjs".into()),
                instructions: "You are the frontend developer. Implement UI components, pages, and client-side logic. Read relevant files first, then implement changes.".into(),
            },
        ],
    };

    if let Ok(json) = serde_json::to_string_pretty(&team) {
        let _ = std::fs::write(&path, json);
    }
}

pub fn list_teams() -> Vec<(String, TeamConfig)> {
    ensure_defaults();
    let dir = teams_dir();
    let mut teams = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    if let Ok(data) = std::fs::read_to_string(&path) {
                        if let Ok(config) = serde_json::from_str::<TeamConfig>(&data) {
                            teams.push((stem.to_string(), config));
                        }
                    }
                }
            }
        }
    }
    teams.sort_by(|a, b| a.1.name.cmp(&b.1.name));
    teams
}

pub fn load_team(slug: &str) -> Option<TeamConfig> {
    ensure_defaults();
    let path = teams_dir().join(format!("{}.json", slug));
    if path.exists() {
        if let Ok(data) = std::fs::read_to_string(&path) {
            if let Ok(config) = serde_json::from_str(&data) {
                return Some(config);
            }
        }
    }
    None
}
