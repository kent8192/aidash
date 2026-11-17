-- Web disclosure approvals and the Research providers are gone (#987, #962):
-- Approval Rules of the `web_search` and `page_reading` kinds judge nothing,
-- and Catalog Approvals of the removed definitions approve nothing. Deleting
-- them is a data change native operations cannot express. Page reading 2.0.0
-- is a new definition, approved afresh like any other.
SET LOCAL aidash.scope = 'deployment';
DELETE FROM public.identity_approval_rules WHERE kind IN ('web_search', 'page_reading');
DELETE FROM public.authorization_catalog WHERE entry_id IN (
    'aidash.research.brave.search',
    'aidash.research.page.find', 'aidash.research.page.view',
    'aidash.research.fixture.search', 'aidash.research.fixture.fetch',
    'aidash.research.fixture.find', 'aidash.research.fixture.view',
    'aidash.bundle.research_brave', 'aidash.bundle.research_fixture');
