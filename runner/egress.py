"""Only operation credentials reach an Execution Pod; Tenant Secrets never do."""


VARIABLES = ('AIDASH_EGRESS_URL',)


def environment(lease):
    if not lease:
        return {}
    base = lease['url'].rstrip('/') + '/v1/' + lease['token'] + '/'
    return {
        # External services only; packages install through the Execution
        # Pod's HTTPS proxy (#745, #729).
        'AIDASH_EGRESS_URL': base,
        'UV_PYTHON_DOWNLOADS': 'never',
        'UV_PYTHON': '/usr/local/bin/python',
    }
