SET LOCAL aidash.scope = 'deployment';
DELETE FROM "identity_approval_rules" WHERE "kind" IN ('web_search', 'page_reading');
DELETE FROM "authorization_catalog" WHERE "entry_id" IN ('aidash.research.brave.search', 'aidash.research.page.find', 'aidash.research.page.view', 'aidash.research.fixture.search', 'aidash.research.fixture.fetch', 'aidash.research.fixture.find', 'aidash.research.fixture.view', 'aidash.bundle.research_brave', 'aidash.bundle.research_fixture');
