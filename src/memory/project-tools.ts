import { tool, createSdkMcpServer } from "@anthropic-ai/claude-agent-sdk";
import { z } from "zod";
import { readdirSync, existsSync, readFileSync, mkdirSync, writeFileSync } from "fs";
import { join } from "path";
import { getConfig, log } from "../config.js";

/**
 * Get the projects root directory.
 * Convention: ~/claude-workspaces/projects/ on this-machine
 */
function getProjectsRoot(): string {
  const cfg = getConfig();
  return cfg.PROJECTS_ROOT;
}

// ── MCP Tools ──

const projectList = tool(
  "project_list",
  "List all available projects. Each project is a directory under the projects root with its own CLAUDE.md and files. Use this to discover what projects exist before delegating work.",
  {},
  async () => {
    const root = getProjectsRoot();

    if (!existsSync(root)) {
      return {
        content: [
          {
            type: "text" as const,
            text: `No projects directory found at ${root}. Ask the user if they want to create one.`,
          },
        ],
      };
    }

    const entries = readdirSync(root, { withFileTypes: true });
    const projects: Array<{ name: string; hasClaude: boolean; description: string }> = [];

    for (const entry of entries) {
      if (!entry.isDirectory()) continue;
      if (entry.name.startsWith(".")) continue;

      const projectPath = join(root, entry.name);
      const claudePath = join(projectPath, "CLAUDE.md");
      const hasClaude = existsSync(claudePath);

      let description = "(no CLAUDE.md)";
      if (hasClaude) {
        const content = readFileSync(claudePath, "utf-8");
        // Extract first non-heading, non-empty line as description
        const lines = content.split("\n").filter((l) => l.trim() && !l.startsWith("#"));
        description = lines[0]?.trim().slice(0, 120) ?? "(empty CLAUDE.md)";
      }

      projects.push({ name: entry.name, hasClaude, description });
    }

    if (projects.length === 0) {
      return {
        content: [
          {
            type: "text" as const,
            text: `Projects directory exists at ${root} but contains no projects.`,
          },
        ],
      };
    }

    const listing = projects
      .map((p) => `• ${p.name}${p.hasClaude ? "" : " ⚠️ no CLAUDE.md"}\n  ${p.description}`)
      .join("\n\n");

    return {
      content: [
        {
          type: "text" as const,
          text: `Found ${projects.length} project(s):\n\n${listing}`,
        },
      ],
    };
  },
);

const projectWork = tool(
  "project_work",
  "Delegate a task to a project-specific agent. This spawns a subagent that runs in the project's directory with its own CLAUDE.md loaded. The subagent has full file access within that project. Use this for coding, editing, research, or any task that needs project-specific context. The subagent's final response is returned to you.",
  {
    project: z
      .string()
      .describe("Name of the project directory (e.g., 'webapp-redesign')"),
    task: z
      .string()
      .describe(
        "Detailed description of the task. Be specific — the subagent only sees this prompt and the project's CLAUDE.md. Include file names, error messages, or context it needs.",
      ),
    max_turns: z
      .number()
      .optional()
      .describe("Max agent loop turns for this task. Default 25."),
  },
  async (args) => {
    const root = getProjectsRoot();
    const projectPath = join(root, args.project);

    if (!existsSync(projectPath)) {
      return {
        content: [
          {
            type: "text" as const,
            text: `Project "${args.project}" not found at ${projectPath}. Use project_list to see available projects, or project_create to make a new one.`,
          },
        ],
      };
    }

    log(
      "info",
      `Delegating to project "${args.project}": ${args.task.slice(0, 100)}...`,
    );

    // The actual subagent delegation happens through the Agent SDK's
    // built-in Agent tool. We return instructions for Claude to use it.
    // Claude will call the Agent tool with cwd set to the project directory.
    return {
      content: [
        {
          type: "text" as const,
          text: [
            `Delegate this task using the Agent tool with the following configuration:`,
            ``,
            `Working directory: ${projectPath}`,
            `Task: ${args.task}`,
            `Max turns: ${args.max_turns ?? 25}`,
            ``,
            `The subagent will load the project's CLAUDE.md automatically.`,
            `Report its final response back to the user.`,
          ].join("\n"),
        },
      ],
    };
  },
);

const projectCreate = tool(
  "project_create",
  "Create a new project directory with a starter CLAUDE.md. Use this when the user wants to start a new project.",
  {
    name: z
      .string()
      .describe(
        "Project directory name (lowercase, hyphens, no spaces — e.g., 'my-new-app')",
      ),
    description: z
      .string()
      .describe("One-line description of the project"),
    type: z
      .enum(["code", "research", "writing", "general"])
      .optional()
      .describe("Project type — determines the starter CLAUDE.md template"),
  },
  async (args) => {
    const root = getProjectsRoot();
    const projectPath = join(root, args.name);

    if (existsSync(projectPath)) {
      return {
        content: [
          {
            type: "text" as const,
            text: `Project "${args.name}" already exists at ${projectPath}.`,
          },
        ],
      };
    }

    // Create directory
    mkdirSync(projectPath, { recursive: true });

    // Generate CLAUDE.md based on type
    const claudeContent = generateClaudeMd(
      args.name,
      args.description,
      args.type ?? "general",
    );
    writeFileSync(join(projectPath, "CLAUDE.md"), claudeContent);

    log("info", `Created project: ${args.name}`);

    return {
      content: [
        {
          type: "text" as const,
          text: `Created project "${args.name}" at ${projectPath} with a starter CLAUDE.md. The project is ready for work delegation.`,
        },
      ],
    };
  },
);

function generateClaudeMd(
  name: string,
  description: string,
  type: string,
): string {
  const base = `# ${name}\n\n${description}\n\n`;

  switch (type) {
    case "code":
      return (
        base +
        `## Project context\n\nThis is a software project.\n\n## Conventions\n\n- Follow existing code style\n- Write tests for new functionality\n- Keep commits atomic and well-described\n\n## Key files\n\n(Add important file paths here as the project grows)\n`
      );
    case "research":
      return (
        base +
        `## Research scope\n\n(Define the research questions here)\n\n## Sources\n\n(Track sources and references here)\n\n## Findings\n\n(Accumulate findings here)\n`
      );
    case "writing":
      return (
        base +
        `## Writing project\n\n(Define the structure, audience, and tone here)\n\n## Outline\n\n(Build the outline here)\n\n## Style notes\n\n(Tone, voice, formatting preferences)\n`
      );
    default:
      return (
        base +
        `## Notes\n\n(Add project context and instructions here)\n`
      );
  }
}

// ── Export ──

export function createProjectMcpServer() {
  return createSdkMcpServer({
    name: "projects",
    version: "1.0.0",
    tools: [projectList, projectWork, projectCreate],
  });
}
