#!/usr/bin/env python3
"""Standalone MCP server for project management."""

import os
import re
from pathlib import Path

from mcp.server.fastmcp import FastMCP

# Default: sibling `projects/` directory next to wherever this server lives
# (i.e. the repo root when installed at workspace/mcp-projects/server.py).
# Override with PROJECTS_ROOT env var for other layouts.
_HERE = Path(__file__).resolve().parent
PROJECTS_ROOT = Path(os.environ.get("PROJECTS_ROOT", str(_HERE.parent.parent / "projects")))
_VALID_NAME = re.compile(r"^[a-z0-9][a-z0-9-]*$")

mcp = FastMCP("projects")


def _validate_project_name(name: str) -> str | None:
    """Return an error message if name is invalid, else None."""
    if not _VALID_NAME.match(name):
        return f'Invalid project name "{name}". Use lowercase letters, numbers, and hyphens only.'
    return None


@mcp.tool()
def project_list() -> str:
    """List all available projects. Each project is a directory under the projects
    root with its own CLAUDE.md and files. Use this to discover what projects exist
    before delegating work."""

    if not PROJECTS_ROOT.is_dir():
        return f"No projects directory found at {PROJECTS_ROOT}. Ask the user if they want to create one."

    projects = []
    for entry in sorted(PROJECTS_ROOT.iterdir()):
        if not entry.is_dir() or entry.name.startswith("."):
            continue

        claude_path = entry / "CLAUDE.md"
        has_claude = claude_path.exists()
        description = "(no CLAUDE.md)"

        if has_claude:
            try:
                content = claude_path.read_text(encoding="utf-8")
                lines = [
                    l.strip()
                    for l in content.split("\n")
                    if l.strip() and not l.strip().startswith("#")
                ]
                description = lines[0][:120] if lines else "(empty CLAUDE.md)"
            except OSError:
                description = "(unreadable CLAUDE.md)"

        warning = "" if has_claude else " ⚠️ no CLAUDE.md"
        projects.append(f"• {entry.name}{warning}\n  {description}")

    if not projects:
        return f"Projects directory exists at {PROJECTS_ROOT} but contains no projects."

    return f"Found {len(projects)} project(s):\n\n" + "\n\n".join(projects)


@mcp.tool()
def project_work(project: str, task: str) -> str:
    """Delegate a task to a project-specific agent. Returns the project path and task
    for Claude to delegate via the Agent tool. The subagent runs in the project's
    directory with its own CLAUDE.md loaded.

    Args:
        project: Name of the project directory (e.g., 'webapp-redesign').
        task: Detailed description of the task. Be specific — the subagent only sees
              this prompt and the project's CLAUDE.md. Include file names, error
              messages, or context it needs.
    """
    if err := _validate_project_name(project):
        return err

    project_path = PROJECTS_ROOT / project

    if not project_path.is_dir() or not project_path.resolve().is_relative_to(PROJECTS_ROOT.resolve()):
        return (
            f'Project "{project}" not found at {project_path}. '
            f"Use project_list to see available projects, or project_create to make a new one."
        )

    return "\n".join([
        "Delegate this task using the Agent tool with the following configuration:",
        "",
        f"Working directory: {project_path}",
        f"Task: {task}",
        "",
        "The subagent will load the project's CLAUDE.md automatically.",
        "Report its final response back to the user.",
    ])


def _generate_claude_md(name: str, description: str, project_type: str) -> str:
    base = f"# {name}\n\n{description}\n\n"

    templates = {
        "code": (
            "## Project context\n\nThis is a software project.\n\n"
            "## Conventions\n\n- Follow existing code style\n"
            "- Write tests for new functionality\n"
            "- Keep commits atomic and well-described\n\n"
            "## Key files\n\n(Add important file paths here as the project grows)\n"
        ),
        "research": (
            "## Research scope\n\n(Define the research questions here)\n\n"
            "## Sources\n\n(Track sources and references here)\n\n"
            "## Findings\n\n(Accumulate findings here)\n"
        ),
        "writing": (
            "## Writing project\n\n(Define the structure, audience, and tone here)\n\n"
            "## Outline\n\n(Build the outline here)\n\n"
            "## Style notes\n\n(Tone, voice, formatting preferences)\n"
        ),
        "general": "## Notes\n\n(Add project context and instructions here)\n",
    }

    return base + templates.get(project_type, templates["general"])


@mcp.tool()
def project_create(name: str, description: str, type: str = "general") -> str:
    """Create a new project directory with a starter CLAUDE.md.

    Args:
        name: Project directory name (lowercase, hyphens, no spaces — e.g., 'my-new-app').
        description: One-line description of the project.
        type: Project type — determines the starter CLAUDE.md template.
              One of: code, research, writing, general. Defaults to general.
    """
    if err := _validate_project_name(name):
        return err

    if type not in ("code", "research", "writing", "general"):
        return f'Invalid project type "{type}". Must be one of: code, research, writing, general.'

    project_path = PROJECTS_ROOT / name

    try:
        project_path.mkdir(parents=True, exist_ok=False)
    except FileExistsError:
        return f'Project "{name}" already exists at {project_path}.'

    claude_content = _generate_claude_md(name, description, type)
    (project_path / "CLAUDE.md").write_text(claude_content, encoding="utf-8")

    return f'Created project "{name}" at {project_path} with a starter CLAUDE.md. The project is ready for work delegation.'


if __name__ == "__main__":
    mcp.run()
