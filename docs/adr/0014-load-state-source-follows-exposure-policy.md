# Load state lives where the Run's Exposure policy puts it

Under `legacy@1` a direct Skill's loaded flag stays in the per-Run skill record; under `deferred@1` all load state lives only in `Run.context.exposure` and the skill record is never written. We first chose one store for both policies, but moving legacy state would change the pinned `skill_load` contract, whose digest every in-flight Run rechecks, so each Run keeps exactly one source chosen by its policy instead (#115).
