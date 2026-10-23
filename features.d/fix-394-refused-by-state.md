## Interfaces

| `fs.bcachefs` namespace verbs (`mkdir`, `rm`, `rmdir`, `mv`, `ln`) refused by the volume's state (a directory that is not empty, a name already there, a missing target, a part of the path that is not a directory): exit 4, `refused`, as the CLI contract has it; `Error::is_state_refusal` tells those refusals from damage | Supported | Unreleased (#394) | #394 | `error::tests::a_state_refusal_is_told_apart_from_damage`, `tests/cli/test-write.sh` |
