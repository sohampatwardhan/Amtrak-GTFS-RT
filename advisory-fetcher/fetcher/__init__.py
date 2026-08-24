"""advisory-fetcher: a standalone browser-backed fetcher for Amtrak service advisories.

Runs a real headless browser to defeat Amtrak's Akamai bot gate, and serves the resulting
advisories HTML over HTTP to the (unchanged) feed-producer service. See the package README.
"""
