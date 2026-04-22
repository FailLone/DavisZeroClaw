use super::*;
use crate::{check_local_config, RuntimePaths};
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

pub(super) fn install_mempalace(paths: &RuntimePaths) -> Result<()> {
    let config = check_local_config(paths)?;
    let package = config.memory_integrations.mempalace.package;
    let python3 = require_command("python3").context("python3 is required to install MemPalace")?;
    let venv_dir = paths.mempalace_venv_dir();
    let python = paths.mempalace_python_path();
    let palace_dir = paths.mempalace_palace_dir();

    fs::create_dir_all(&paths.runtime_dir)?;
    if !python.is_file() {
        println!("Creating MemPalace venv: {}", venv_dir.display());
        run_status(
            Command::new(&python3)
                .arg("-m")
                .arg("venv")
                .arg(&venv_dir)
                .env("PATH", tool_path_env())
                .current_dir(&paths.repo_root),
            "python3 -m venv .runtime/davis/mempalace-venv",
        )?;
    } else {
        println!("MemPalace venv already exists: {}", venv_dir.display());
    }

    println!("Upgrading pip.");
    run_status(
        Command::new(&python)
            .arg("-m")
            .arg("pip")
            .arg("install")
            .arg("--upgrade")
            .arg("pip")
            .env("PATH", tool_path_env())
            .current_dir(&paths.repo_root),
        "mempalace pip upgrade",
    )?;

    println!("Installing MemPalace package: {package}");
    run_status(
        Command::new(&python)
            .arg("-m")
            .arg("pip")
            .arg("install")
            .arg("--upgrade")
            .arg(&package)
            .env("PATH", tool_path_env())
            .current_dir(&paths.repo_root),
        "pip install mempalace",
    )?;

    fs::create_dir_all(&palace_dir)?;
    println!("MemPalace installed.");
    println!("Python: {}", python.display());
    println!("Palace: {}", palace_dir.display());
    println!("Next: daviszeroclaw memory mempalace enable");
    Ok(())
}

pub(super) fn enable_mempalace(paths: &RuntimePaths) -> Result<()> {
    let config_path = paths.local_config_path();
    let raw = fs::read_to_string(&config_path)
        .with_context(|| format!("failed to read {}", config_path.display()))?;
    let section = r#"[memory_integrations.mempalace]
enabled = true
python = ""
palace_dir = ""
package = "mempalace"
tool_timeout_secs = 30
"#;
    let updated = replace_toml_section(&raw, "[memory_integrations.mempalace]", section);
    fs::write(&config_path, updated)
        .with_context(|| format!("failed to write {}", config_path.display()))?;
    println!("MemPalace enabled in {}", config_path.display());
    println!("Next: daviszeroclaw memory mempalace check");
    Ok(())
}

pub(super) fn check_mempalace(paths: &RuntimePaths) -> Result<()> {
    let config = check_local_config(paths)?;
    let mempalace = config.memory_integrations.mempalace;
    let python = if mempalace.python.trim().is_empty() {
        paths.mempalace_python_path()
    } else {
        PathBuf::from(mempalace.python.trim())
    };
    let palace_dir = if mempalace.palace_dir.trim().is_empty() {
        paths.mempalace_palace_dir()
    } else {
        PathBuf::from(mempalace.palace_dir.trim())
    };

    println!("MemPalace config:");
    println!("- enabled: {}", mempalace.enabled);
    println!("- python: {}", python.display());
    println!("- palace_dir: {}", palace_dir.display());
    println!("- package: {}", mempalace.package);
    println!("- tool_timeout_secs: {}", mempalace.tool_timeout_secs);

    if !mempalace.enabled {
        bail!("MemPalace is not enabled. Run: daviszeroclaw memory mempalace enable");
    }
    if !python.is_file() {
        bail!(
            "MemPalace Python was not found: {}\nRun: daviszeroclaw memory mempalace install",
            python.display()
        );
    }
    fs::create_dir_all(&palace_dir)?;

    let import_check = command_output(
        Command::new(&python)
            .arg("-c")
            .arg("import mempalace; print('mempalace import ok')")
            .env("PATH", tool_path_env())
            .current_dir(&paths.repo_root),
    )?;
    print_command_streams(&import_check.stdout, &import_check.stderr);
    if !import_check.status_success {
        bail!("MemPalace package import failed");
    }

    let help_check = command_output(
        Command::new(&python)
            .arg("-m")
            .arg("mempalace.mcp_server")
            .arg("--help")
            .env("PATH", tool_path_env())
            .current_dir(&paths.repo_root),
    )?;
    if !help_check.status_success {
        print_command_streams(&help_check.stdout, &help_check.stderr);
        bail!("MemPalace MCP server did not respond to --help");
    }

    println!("MemPalace MCP server is available.");
    println!("Running MemPalace MCP smoke test.");
    let smoke_check = command_output(
        Command::new(&python)
            .arg("-c")
            .arg(MEMPALACE_SMOKE_TEST_SCRIPT)
            .arg(&palace_dir)
            .env("PATH", tool_path_env())
            .current_dir(&paths.repo_root),
    )?;
    print_command_streams(&smoke_check.stdout, &smoke_check.stderr);
    if !smoke_check.status_success {
        bail!("MemPalace MCP smoke test failed");
    }

    println!("Restart Davis to render the MCP server into ZeroClaw config.");
    Ok(())
}

const MEMPALACE_SMOKE_TEST_SCRIPT: &str = r#"
import json
import sqlite3
import subprocess
import sys
import time
from pathlib import Path

palace = Path(sys.argv[1])
marker = f"davis_mempalace_check_{int(time.time())}"
drawer_content = f"{marker}: temporary MemPalace MCP check drawer. Delete after verification."
diary_content = f"{marker}: temporary MemPalace MCP check diary entry. Delete after verification."
subject = f"{marker}_subject"

proc = subprocess.Popen(
    [sys.executable, "-m", "mempalace.mcp_server", "--palace", str(palace)],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
    text=True,
    bufsize=1,
)
next_id = 1

def request(method, params=None):
    global next_id
    payload = {"jsonrpc": "2.0", "id": next_id, "method": method}
    if params is not None:
        payload["params"] = params
    next_id += 1
    proc.stdin.write(json.dumps(payload) + "\n")
    proc.stdin.flush()
    line = proc.stdout.readline()
    if not line:
        stderr = proc.stderr.read()
        raise RuntimeError(f"MemPalace MCP server returned no response: {stderr}")
    response = json.loads(line)
    if "error" in response:
        raise RuntimeError(response["error"])
    return response

def tool(name, arguments=None):
    response = request("tools/call", {"name": name, "arguments": arguments or {}})
    text = response["result"]["content"][0]["text"]
    return json.loads(text)

def ensure(condition, message):
    if not condition:
        raise RuntimeError(message)

def cleanup_kg():
    db = palace / "knowledge_graph.sqlite3"
    if not db.is_file():
        return
    with sqlite3.connect(db) as conn:
        conn.execute("delete from triples where subject = ? or object = ?", (subject, subject))
        conn.execute("delete from entities where id = ?", (subject,))
        conn.commit()

drawer_id = None
diary_id = None
try:
    request("initialize", {
        "protocolVersion": "2025-11-25",
        "capabilities": {},
        "clientInfo": {"name": "daviszeroclaw-mempalace-check", "version": "0"},
    })
    tools = request("tools/list")["result"]["tools"]
    tool_names = {tool["name"] for tool in tools}
    required = {
        "mempalace_status",
        "mempalace_search",
        "mempalace_add_drawer",
        "mempalace_delete_drawer",
        "mempalace_diary_write",
        "mempalace_diary_read",
        "mempalace_kg_add",
        "mempalace_kg_query",
        "mempalace_kg_invalidate",
    }
    missing = sorted(required - tool_names)
    ensure(not missing, f"missing required tools: {', '.join(missing)}")

    status_before = tool("mempalace_status")

    added = tool("mempalace_add_drawer", {
        "wing": "davis",
        "room": "smoke-test",
        "content": drawer_content,
    })
    ensure(added.get("success"), f"add_drawer failed: {added.get('error')}")
    drawer_id = added.get("drawer_id")
    ensure(drawer_id, "add_drawer did not return drawer_id")

    search = tool("mempalace_search", {"query": marker, "limit": 3})
    results_text = json.dumps(search, ensure_ascii=False)
    ensure(drawer_content in results_text, "search did not return the smoke-test drawer")

    deleted = tool("mempalace_delete_drawer", {"drawer_id": drawer_id})
    ensure(deleted.get("success"), f"delete_drawer failed: {deleted.get('error')}")
    drawer_id = None

    search_after_delete = tool("mempalace_search", {"query": marker, "limit": 3})
    remaining = search_after_delete.get("results") or []
    ensure(
        not any(drawer_content in json.dumps(item, ensure_ascii=False) for item in remaining),
        "deleted drawer still appears in search results",
    )

    diary = tool("mempalace_diary_write", {
        "agent_name": "davis",
        "topic": "smoke-test",
        "entry": diary_content,
    })
    ensure(diary.get("success"), f"diary_write failed: {diary.get('error')}")
    diary_id = diary.get("entry_id")
    ensure(diary_id, "diary_write did not return entry_id")

    diary_read = tool("mempalace_diary_read", {"agent_name": "davis", "last_n": 5})
    ensure(diary_content in json.dumps(diary_read, ensure_ascii=False), "diary_read did not return the smoke-test entry")

    diary_delete = tool("mempalace_delete_drawer", {"drawer_id": diary_id})
    ensure(diary_delete.get("success"), f"delete diary entry failed: {diary_delete.get('error')}")
    diary_id = None

    kg_add = tool("mempalace_kg_add", {
        "subject": subject,
        "predicate": "check_predicate",
        "object": "check_object",
        "valid_from": "2026-04-17",
    })
    ensure(kg_add.get("success"), f"kg_add failed: {kg_add.get('error')}")

    kg_query = tool("mempalace_kg_query", {"entity": subject, "direction": "both"})
    ensure(kg_query.get("count", 0) >= 1, "kg_query did not return the smoke-test fact")

    kg_invalidate = tool("mempalace_kg_invalidate", {
        "subject": subject,
        "predicate": "check_predicate",
        "object": "check_object",
        "ended": "2026-04-17",
    })
    ensure(kg_invalidate.get("success"), f"kg_invalidate failed: {kg_invalidate.get('error')}")
    cleanup_kg()

    status_after = tool("mempalace_status")
    ensure(status_after.get("protocol"), "mempalace_status did not return Memory Protocol after smoke test")

    before_drawers = status_before.get("total_drawers")
    after_drawers = status_after.get("total_drawers")
    print("MemPalace MCP smoke test ok.")
    print(f"- tools: {len(tool_names)} available")
    print(f"- drawer/search/delete: ok")
    print(f"- diary write/read/delete: ok")
    print(f"- KG add/query/invalidate: ok")
    print(f"- Memory Protocol: ok")
    if before_drawers is not None and after_drawers is not None:
        print(f"- total_drawers: {before_drawers} -> {after_drawers}")
except Exception as exc:
    print(f"MemPalace MCP smoke test failed: {exc}", file=sys.stderr)
    print("Hint: if the error mentions SSL, handshake, or ONNX, remove a corrupt Chroma model cache and retry:", file=sys.stderr)
    print("  rm -f ~/.cache/chroma/onnx_models/all-MiniLM-L6-v2/onnx.tar.gz", file=sys.stderr)
    raise
finally:
    try:
        if drawer_id:
            tool("mempalace_delete_drawer", {"drawer_id": drawer_id})
    except Exception:
        pass
    try:
        if diary_id:
            tool("mempalace_delete_drawer", {"drawer_id": diary_id})
    except Exception:
        pass
    try:
        cleanup_kg()
    except Exception:
        pass
    proc.terminate()
    try:
        proc.wait(timeout=3)
    except subprocess.TimeoutExpired:
        proc.kill()
"#;

pub(super) fn replace_toml_section(raw: &str, header: &str, replacement: &str) -> String {
    let lines = raw.lines().collect::<Vec<_>>();
    let Some(start) = lines.iter().position(|line| line.trim() == header) else {
        let mut output = raw.trim_end().to_string();
        output.push_str("\n\n");
        output.push_str(replacement.trim_end());
        output.push('\n');
        return output;
    };
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find_map(|(index, line)| {
            let trimmed = line.trim();
            (trimmed.starts_with('[') && trimmed.ends_with(']')).then_some(index)
        })
        .unwrap_or(lines.len());

    let mut output = String::new();
    for line in &lines[..start] {
        output.push_str(line);
        output.push('\n');
    }
    output.push_str(replacement.trim_end());
    output.push('\n');
    for line in &lines[end..] {
        output.push_str(line);
        output.push('\n');
    }
    output
}

