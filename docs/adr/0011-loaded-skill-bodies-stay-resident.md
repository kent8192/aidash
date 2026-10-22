# Loaded Skill bodies stay resident until unloaded

A loaded Skill body stays in the instructions channel of every later request, counts against the Skill budget, and is never removed by compaction; only an explicit unload removes it, and loading an already resident Skill returns its identity and digest instead of the body. We rejected returning bodies as compactable tool results because compaction could then silently drop instructions the model depends on and reloads would duplicate them (#115).
