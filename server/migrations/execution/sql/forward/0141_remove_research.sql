-- Research is no longer a category (#987, ADR 0138): page reading keeps no
-- source, snapshot, fragment, citation, attempt or journal result, and
-- Evidence Revocation, Research account terms and Provider results are gone.
-- A typed DropTable cannot drop the row policies or reverse to the composite
-- keys and partial indexes, so the drop and its reversal are SQL. The Web
-- disclosure approval records were deleted by the preceding operation.
DROP TABLE public.research_page_images, public.research_originals,
    public.research_fragments, public.research_snapshots,
    public.research_sources, public.research_attempts,
    public.research_journal_results, public.research_revocations,
    public.research_account_terminations;
