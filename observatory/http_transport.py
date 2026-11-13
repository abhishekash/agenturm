"""Receiver requests never forward credentials or payloads through redirects."""

import urllib.request
import urllib.error


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, msg, headers, newurl):
        raise urllib.error.HTTPError(
            request.full_url, code, "Receiver redirects are not supported", headers, fp
        )


class HTTPSOnDemand(urllib.request.HTTPSHandler):
    """Keep local HTTP polls from repeatedly loading the system TLS trust store.

    build_opener normally constructs HTTPSHandler even for an HTTP request.
    Delegate actual HTTPS requests to the standard handler, preserving Python's
    default certificate/hostname verification and protocol configuration.
    Each opener remains independent; no credentials or mutable handlers are shared.
    """
    def __init__(self):
        urllib.request.AbstractHTTPHandler.__init__(self)

    def https_open(self, request):
        handler = urllib.request.HTTPSHandler()
        handler.add_parent(self.parent)
        return handler.https_open(request)


def urlopen(request, *, timeout):
    return urllib.request.build_opener(NoRedirect(), HTTPSOnDemand()).open(request, timeout=timeout)
