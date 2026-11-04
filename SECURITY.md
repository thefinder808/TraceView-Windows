# Security

## Reporting a vulnerability

Please don't open a public issue for a security problem. Report it privately through GitHub instead: on this repository's **Security** tab, choose **Report a vulnerability**. You'll get a reply there.

Only the latest release gets fixes.

## What counts

TraceView reads files that anyone able to write to them controls, so these matter most:

- **A crafted log** that crashes TraceView, hangs it, makes it use far too much memory, or runs code.
- **A shared settings, theme or log format file** that does any of those.
- **The command line's JSON** carrying log text that could be mistaken for something other than data. It's marked as untrusted.
- **The signed release:** anything that would let someone else's code be signed as TraceView, or a release that doesn't match its checksums.

## Checking a download

Every release EXE is signed by *Nathaniel Graham* through Microsoft's Artifact Signing, and timestamped. Properties > Digital Signatures shows it. Each release also lists the SHA-256 of its zips in `SHA256SUMS.txt`. An EXE that isn't signed, or is signed by anyone else, didn't come from here.
