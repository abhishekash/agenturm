# Agenturm Rust 0.3.14

This update reorganizes the task roster into distinct navigation, filter, task and preview regions, with quieter metadata and fewer repeated lines.

## A calmer task roster

- Workspace destinations sit in their own outlined menu. Time scope and recorded-state filters have separate rows, so navigation and filtering are easier to distinguish.
- Ordinary tasks use one row. Provider-only subrows are removed; secondary context remains when it distinguishes a task or agent. Ready, inactive and unknown states use quieter styling.
- Project headings use short, disambiguated paths. The full recorded project remains available in the inspector and contextual help.
- The selected-task preview emphasizes the task, recorded state, project and recent activity, with direct actions for the overview, prompt history and session activity.
- The footer shows primary shortcuts and concise collection status. Press `?` for the complete collection notice, scope details and keyboard reference.

## Smaller terminals

At 80×24 and in a 60-column split, the task list uses the full width. Press Enter to inspect a task and Esc to return to the same selection. The active state filter stays visible; Tab changes it. Additional destinations remain available through `:` commands and `?` help.

The task roster supports terminals down to 60×18. Below that size, it displays the required dimensions. Contours, labels and selection markers remain meaningful without color.

The roster continues to show recorded session states and counts for loaded history. Prompt-history permissions and the selected-agent Activity, MCP and Tools behavior introduced in 0.3.12 remain available.

## Upgrade

After the 0.3.14 installer pin is published, repeat:

```sh
curl -fsSL https://www.agenturm.dev/install.sh | bash
agenturm
```

Close an already-running older Agenturm process before opening the updated command. Installing an update does not replace a running process.
