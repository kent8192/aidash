# Loaded capabilities are native tools, not a bridge

Under a deferred Exposure policy, loading a capability adds its exact pinned schema to later requests' native tool list under its Model alias, and dispatch additionally requires the alias to be in the Run's persisted Exposure set. We rejected a fixed `capability_invoke{alias, arguments}` bridge, even though it keeps the tool list stable for provider prompt caching, because it gives up provider-side schema enforcement and typed arguments; revisit only with measured evidence (#115).
