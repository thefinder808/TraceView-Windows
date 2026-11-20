# Security

## Reporting a vulnerability

Please don't open a public issue for a security problem. Report it privately through GitHub instead: on this repository's **Security** tab, choose **Report a vulnerability**. You'll get a reply there.

Only the latest release gets fixes.

## What counts

TraceView reads files that anyone able to write to them controls, so these matter most:

- **A crafted log** that crashes TraceView, hangs it, makes it use far too much memory, or runs code.
- **A shared settings, theme or log format file** that does any of those.
- **A crafted `CMTrace.exe`** that does any of those. Error Lookup reads one as data, never running it, from ConfigMgr's folders or from beside `traceview.exe`.
- **The command line's JSON** carrying log text that could be mistaken for something other than data. It's marked as untrusted.
- **The signed release:** anything that would let someone else's code be signed as TraceView, or a release that doesn't match its checksums.
- **Updates:** anything that would get something installed that isn't a newer TraceView signed by the same publisher as the copy running, or make a check send more than this PC's address and TraceView's version.

## Checking a download

Every release EXE is signed by *Nathaniel Graham* through Microsoft's Artifact Signing, and timestamped. Properties > Digital Signatures shows it. Each release also lists the SHA-256 of its zips, and from 0.13 its EXEs, in `SHA256SUMS.txt`. An EXE that isn't signed, or is signed by anyone else, didn't come from here.

## How updates are checked

The installed copy's updates make these checks before replacing anything, and change nothing if one fails:

1. The download came over HTTPS from GitHub, or the server GitHub sends release downloads to.
2. Its SHA-256 matches its line in the release's `SHA256SUMS.txt`.
3. Windows trusts its signature, with revocation checked for the whole certificate chain.
4. It's signed by the same publisher, chaining to the same root, as the copy that's running. That copy's own signature is the reference, so nothing is hard-coded and a certificate in the same name from another authority isn't enough.
5. The version inside it is the one its name gives, and newer than the copy running. Pre-releases and drafts are never offered.

The file stays open against changes from the moment it's checked until it's renamed into place. Anything running as you could already replace `traceview.exe`, so these checks guard the download, not the folder. Taking over the GitHub repository isn't enough to ship an update: a release is signed only once its owner approves it, and the signature is what's checked.
