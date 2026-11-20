# Pi requested command Activity preview

## Behavior

The interactive cockpit registers a trusted Pi command preview provider at its
composition root. It offers **z Check** only on a content-free, projected plugin
Activity row for a **started shell operation** with a recorded operation ID.
The local `/api/plugin-command-content` route decides whether the selected
event actually has an eligible, current Pi requested command. A successful
response is labeled **Requested shell command · redacted**; it does not imply
that the command ran or succeeded. Capture remains opt-in.

The UI keeps the returned redacted text only in transient inspector state. It
expires after 20 seconds, rechecks receiver revision every 2 seconds, and is
removed when selection, receiver, provider, or policy revision changes. The
projected Activity row, copy text, Review, and investigation export contain no
command body. The route and its exact source proof are delivered separately.

## Validation

- A real admitted Pi shell call projected to the required content-free row;
  the row contained no command text.
- Mocked route responses were accepted only with the selected event, receiver,
  unchanged three-part revision, and safe bounded redacted text.
- A worker test confirmed the provider requests only the local command-content
  route. A revocation recheck cleared visible text.
- An authenticated loopback server delivered a real exact Pi command to the
  Textual Activity inspector after `z`; capture revocation removed the visible
  command on recheck.
- Headless inspectors at **60×18** and **80×24** retained the header and showed
  the explicit `z` action, available label, expiry, and selection clearing.
- The affected UI and content delivery suites, including the command route,
  passed together after rebasing onto the route implementation: **33 tests**.

## Scope

The provider uses the existing preview registry. No central inspector logic or
preview lifecycle was changed. It does not add a second content channel,
clipboard path, Review field, or command execution claim.
