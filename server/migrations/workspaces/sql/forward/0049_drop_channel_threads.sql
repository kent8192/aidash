-- Reply threads become top-level Threads (#1008, ADR 0140). Aidash is
-- undeployed, so the reply-thread rows are dropped, not converted: replies stay
-- as flat messages of their Thread. The table carries a forced row-level
-- policy and a write guard that typed DropTable cannot drop or restore.
DROP TABLE public.channel_threads;
