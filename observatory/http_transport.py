"""Receiver requests never forward credentials or payloads through redirects."""

import urllib.request
import urllib.error


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, msg, headers, newurl):
        raise urllib.error.HTTPError(
            request.full_url, code, "Receiver redirects are not supported", headers, fp
        )


def urlopen(request, *, timeout):
    return urllib.request.build_opener(NoRedirect()).open(request, timeout=timeout)
