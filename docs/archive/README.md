# Archive

Historical design, planning, and orchestration documents for evosim v1. Nothing here should be edited. The canonical spec is PITCH-v5 + PITCH-v6; v6 overrides v5 on any conflict.

## PITCH-v1 through PITCH-v4

Early iterations of the design. v1 is the seed idea; each revision added mechanical detail. They are superseded entirely by v5 and are kept for lineage reference only.

## PITCH-v5.md

The primary design document for v1. All game mechanics, tick ordering (§3.5), genome schema (§4), NN shape (§5), energy economy (§7), species detection (§12), persistence (§13), UI (§11), and acceptance criteria (§16) are defined here. When in doubt, this is the spec.

## PITCH-v6.md

A patch document on top of v5. Contains §A (stack decisions), §B (render), §C (camera), §D (world shape), §E (NN details), §H (naming), §I (save format), §J (chunking), §K (slider list and binding defaults), §L (HoF definitions), §M (snapshot hash order), and §N (pan limits). v6 overrides v5 on any point where they conflict.

## ORCHESTRATOR.md

The build brief given to the orchestrator agent that constructed v1. Describes the milestone structure (A–F), subagent roles (planner, implementer, reviewer), and hand-off protocol. Useful context if you want to understand why the code is structured the way it is, or if you want to run a similar orchestrated build for a future version.

## plans/

One file per milestone (B through F — A had no plan doc). Each file contains the planner's intent, the implementer's task list, and the reviewer's findings. Milestone-F.md also contains the F.30 balance-tuning notes. Reading these in order (B → F) shows how the build proceeded and why certain decisions were made.

## original_idea_docs/

Pre-spec notes from the human author: pace ideas, formula sketches, early game-rule drafts. These predate the formal PITCH series and are purely for historical interest.
