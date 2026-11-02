"""Only operation credentials reach an Execution Pod; Tenant Secrets never do."""


from urllib.parse import urlsplit

VARIABLES = ('AIDASH_EGRESS_URL', 'NPM_CONFIG_REGISTRY', 'NPM_CONFIG_REPLACE_REGISTRY_HOST',
             'UV_INDEX_URL', 'UV_INSECURE_HOST')


def environment(lease):
    if not lease:
        return {}
    base = lease['url'].rstrip('/') + '/v1/' + lease['token'] + '/'
    return {
        'AIDASH_EGRESS_URL': base,
        'NPM_CONFIG_REGISTRY': base + 'core.npm/',
        # The gateway rewrites index tarball links onto its own paths; `npmjs`
        # redirects only registry.npmjs.org links (lockfiles) and leaves those
        # rewritten links alone, which `always` would prefix twice (#679).
        'NPM_CONFIG_REPLACE_REGISTRY_HOST': 'npmjs',
        'UV_INDEX_URL': base + 'core.pypi/simple/',
        'UV_INSECURE_HOST': urlsplit(lease['url']).hostname,
        'UV_PYTHON_DOWNLOADS': 'never',
        'UV_PYTHON': '/usr/local/bin/python',
    }
