# Third-party sources

IndexLoom is an independent implementation. The local development workspace may
contain reference checkouts of Jackett, Prowlarr, and Prowlarr/Indexers, but
they are outside this Git repository and are never included in release images.

The separate `IndexLoom/Indexers` repository contains deterministic,
source-derived Cardigann definitions where licensing permits redistribution.
It records exact source revisions, attribution, source and semantic hashes, and
the retained license. Runtime import adapters must apply the same provenance
rules. Contributors must not copy upstream application source into IndexLoom.
