# HTTP validation in shallow CI checkouts

GitHub CI checks out one commit by default. The HTTP smoke attempted to resolve the historical baseline commit in the harness repository before it started either receiver, even though smoke intentionally accepts arbitrary current source trees. This raised Git exit 128 in a real depth-one diagnostic clone and left no paired writer result; the old test then obscured the cause with `KeyError: writer_inputs_equal`.

The baseline is now pinned by its full canonical SHA, `5e0c39786066fb88c0e270418573e361aeb6cf9a`, without requiring that historical Git object in the harness checkout. Controlled runs still require that exact application head, an explicit full candidate SHA, clean frozen sources and the registered generated corpus. Smoke does not resolve or validate historical versions and remains nonpublishable. Missing loaded results now report the retained startup errors directly.

The focused contract asserts smoke performs no historical Git lookup; controlled registration still retains exact B/C roles, full candidate validation and balanced ordering. Existing authentication, namespace, schema, 503/deadline retention, source invariance, cleanup and nonpublishable-smoke checks remain unchanged. An independent read-only peer reproduced the old registration failure in a real depth-one clone and found no blocker in using the full SHA literal.

The first touched harness gate passed 15 tests in 22.369 seconds before the equivalent full-literal refinement. Final shallow-source functional verification and new-head integration CI are separate gates. Main91 CI failures remain recorded; no previous failed run is relabelled green. No controlled resource run or performance result is claimed here.
