# Prompt signals capability proof

This trusted, opt-in Python provider adds a pane through Agenturm's generic `PaneContribution` constructor hook. The host reads an exact service, session and canonical agent scope from indexed Activity, reduces it to `PromptSignalFacts`, and passes only those counts to the provider. No prompt text, shell command, path or raw event reaches the pane code. A caller must supply a current capture decision; denied or incomplete scope returns an unavailable model without querying Activity.

The pane reports recorded user messages, recorded shell actions and the time from the first recorded user message to the first later recorded shell action. These are observational signals, **not a prompt quality or efficacy score**. The denominator is the number of events in a bounded recent window of at most 200; a paginated or full window is marked partial. Missing tool evidence is shown as unavailable, not zero milliseconds.

Example trusted integration in a host that already owns the Store and current capture policy:

```python
from examples.prompt_signals_capability.provider import pane
from observatory.capability_facts import prompt_signal_facts

contribution = pane(lambda scope: prompt_signal_facts(
    store, scope, capture_allowed=current_capture_decision(scope)))
app = ObservatoryApp(feed, ui_panes=(contribution,))
```

The generic shell renders this pane at 60×18 without a new view dispatch branch. It is not external code discovery or process isolation. The provider runs synchronously in the trusted UI process; a production capability host still needs supervised execution, deadlines, explicit grants and policy-current read certification.
