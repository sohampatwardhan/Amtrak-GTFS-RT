"""The browser worker: one headless Chromium that reads the two Amtrak sources plain HTTP cannot.

``www.amtrak.com`` sits behind Akamai, which resets a plain HTTP client. The worker runs a real
browser to load Amtrak's train-status documents (:mod:`worker.tracks`) and the Service Alerts &
Notices pages (:mod:`worker.notices`), and serves both to the Rust service on ``127.0.0.1``.
"""
