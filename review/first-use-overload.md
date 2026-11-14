# Agenturm first use overload

User feedback changes the immediate priority. Some people tried Agenturm and became lost in the information. That is direct evidence of a usability problem. It does not yet distinguish confusing terminology, poor prioritization, difficult navigation or an absent need for the product. The repository's empty pilot sheets do not invalidate these trials.

The first experiment should reduce the concepts needed to complete one useful task. Adding another intelligence panel would increase the number of places users need to understand.

## What the current structure asks users to learn

At main `7c80dc3`, the documented navigation has four primary views: Agents, Activity, MCP Calls and Traces, plus Host, Metrics and Findings under additional navigation. The session inspector has Overview, Prompts and Details, with separate routes for Review, attention and saved tasks. The inspector defines six action buttons: Review, Activity, Trace, Mark reviewed, Dismiss and AI summary. Visibility varies by state and terminal size; these are route counts, not a claim that every control appears at once.

A newcomer has to determine which representation answers their question. In particular, Activity, Traces, Review and Findings describe different ways of inspecting related work. Making those distinctions before experiencing a benefit is a plausible source of overload. This is a structural diagnosis from code and documentation, not an observation of the reported participants.

## A bounded design experiment

Use the existing landing workflow to show the task, its recorded state and the reason it deserves attention. Test three groups: Needs input, Ready for review and Working. Keep working sessions easy to inspect; an attention-only view could hide the monitoring value users came for.

On selecting a task, show one short reason, its evidence and the relevant next step. For an explicit question, show the question and how to find the native session. For a reported completion, show recorded checks and subsequent changes. Preserve unknown or incomplete coverage beside the affected conclusion. A normal quiet state should say no recorded requests need attention; it must not imply that the code is correct.

Move raw Activity, MCP, Trace, usage and source identity details behind contextual investigation actions. Keep history search discoverable. Remove the global technical-view tab strip from the proposed entry screen. Remove the default multi-action inspector bar there; show the primary relevant action and an obvious route to further evidence. Saved tasks can be introduced when someone chooses to continue a task.

These changes would reorganize existing capabilities. They do not require a new eighth view or another interpretation model.

## Wide and narrow sketches

Illustrative data only. The first sketch is the wide split; the second is the single-pane fallback.

```text
120 columns                                  60 columns
Agenturm                                     Agenturm
2 need input | 1 ready | 4 working            2 need input | 1 ready | 4 working

Tasks                  Selected task         NEEDS INPUT
NEEDS INPUT            Authentication fix    > Authentication fix
> Authentication fix   Codex / project-a        Codex / project-a
  Choose API behavior                          Choose API behavior
  Migration review     Recorded question
                       Change public API?      Migration review
READY FOR REVIEW
  Retry fix            Find native session   READY FOR REVIEW
                       Inspect evidence        Retry fix

Enter Inspect  / Search  ? Help  q Quit       Enter Inspect  / Search  ? Help  q Quit
```

At 80 by 24, use the same single-pane model: Enter opens the selected item and Esc returns, retaining selection. At 60 by 18, keep that behavior, shorten secondary context and scroll the list. Below 60 by 18, preserve the existing truthful resize message. The wide preview is optional space, not a prerequisite for completing a task.

The proposed entry screen has zero panel borders, no marker on every row, one selection marker and one written state per group. It removes seven technical destinations from the entry-level tab strip while retaining investigation access. Use at most one color cue in addition to the state text. Chrome-to-data cell ratios, current rendered border depth and current redundant row signals have not been measured in this structural pass; verify them on captured frames before implementing cosmetic changes. Current CSS already removes several borders and hides some statistics, so additional spacing alone is unlikely to resolve the conceptual problem.

## Test the change against an actual task

Give returning participants the same kind of work they previously struggled with. Ask them to find a waiting agent, explain the reason it needs them and locate the appropriate next action without coaching. Separately ask them to inspect one completion and identify what the recorded evidence does and does not establish. Compare the current interface with the reduced version, varying order to limit learning effects.

Record wrong turns, explanations users give, help required and completion time. After the task, establish whether it solved a problem they actually encounter. A simpler screen can improve navigation while still failing to create a reason to use the tool. Do not infer adoption from task completion alone.

The immediate recommendation is to test a smaller default workflow before expanding intelligence. The pending clarification is where users got lost: deciding what needed attention, understanding the information or finding the next action.
