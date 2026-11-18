//! Short explanations, in TraceView's own words, of common codes that Windows
//! has no text for: ConfigMgr's, which only CMTrace explains, and the
//! servicing codes in `cbs_names.rs`. They're written for TraceView, not
//! copied, and used only when neither Windows nor CMTrace has text for the
//! code, so a PC that logs are copied to still says something useful.
//!
//! Each ConfigMgr explanation was checked against what CMTrace 5.00 says the
//! code means. Each servicing one follows from the code's name and Microsoft
//! Learn's pages listed in `cbs_names.rs`. Add one only when its meaning is
//! known that way, and write it fresh rather than copying anyone's text.

/// Codes and what they mean, sorted by code.
pub(crate) const EXPLAINED: &[(u32, &str)] = &[
    // Servicing
    (0x800F0805, "The update package isn't a valid one for this Windows."),
    (0x800F080D, "An item in a package's manifest isn't valid."),
    (0x800F081F, "Files a package or feature needs weren't found, in the component store or in any source given."),
    (0x800F0821, "The program that started the servicing operation stopped it before it finished."),
    (0x800F0825, "This package can't be removed."),
    (0x800F0830, "This Windows image can't be serviced in the state it's in."),
    (0x800F0831, "The component store, where Windows keeps the files it services, is damaged."),
    (0x800F0900, "Servicing couldn't read an XML file, such as a package's manifest."),
    (0x800F0904, "Servicing found more than one active Windows edition, where there should be one."),
    (0x800F0905, "Servicing found no active Windows edition."),
    (0x800F0911, "Source files changed from what servicing expected."),
    (0x800F0920, "Servicing stopped responding during the operation."),
    (0x800F0922, "An installer that an update runs as part of applying it failed."),
    (0x800F0982, "The component an update's changes are for wasn't found on this PC."),
    (0x800F0984, "A file that an update's changes apply to is missing from this PC."),
    (0x800F0985, "Rolling a file back to its base version, before updating it, failed."),
    (0x800F0986, "Applying an update's changes to a file failed."),
    (0x800F0987, "Building a whole file from an update's null delta, which holds no changes, failed."),
    (0x800F0988, "An update's file changes can't be combined with the versions already on this PC."),
    (0x800F0989, "What's needed to roll a file back to its base version, before updating it, is missing."),
    (0x800F0991, "A file an update needs is missing from the update or the component store."),
    // ConfigMgr
    (0x87D00213, "An operation ran out of time before it finished."),
    (0x87D00215, "Something ConfigMgr looked up wasn't there."),
    (0x87D00222, "Something cancelled the operation before it finished."),
    (0x87D00227, "The feature this needed is turned off."),
    (0x87D00244, "A part of the client was used before it had started up."),
    (0x87D00260, "A job was asked to do something it can't do in the state it's in."),
    (0x87D00269, "The client couldn't find a management point it needed."),
    (0x87D0027C, "Downloading configuration item documents, the definitions behind deployments, ran out of time."),
    (0x87D0027D, "Downloaded configuration item documents failed their hash check."),
    (0x87D0027E, "A server answered with an HTTP error status."),
    (0x87D00281, "No certificate on this PC matched what the client was told to use."),
    (0x87D00321, "A script ran past its time limit."),
    (0x87D00324, "The install finished, but the application's detection method still doesn't find it, so the install counts as failed."),
    (0x87D00325, "The uninstall finished, but the application's detection method still finds it."),
    (0x87D00326, "WMI on this PC has no record of the application deployment asked about."),
    (0x87D00327, "A script wasn't signed, and the client's settings require signed scripts."),
    (0x87D00601, "A scan for updates took too long."),
    (0x87D00607, "The content a deployment needs couldn't be found, as when no distribution point the client can use has it."),
    (0x87D00664, "A software update job was cancelled before it finished."),
    (0x87D00665, "A software update job had no updates left to work on."),
    (0x87D00668, "An update was applied, but a scan still finds it needed."),
    (0x87D00692, "A Group Policy setting for Windows Update conflicts with what ConfigMgr needs, as when it points the PC at another update server."),
    (0x87D0070C, "An update took longer to install than the deployment allows."),
    (0x87D01106, "The program to run couldn't be checked, or its command line couldn't be built, as when the file it names isn't in its content."),
    (0x87D01107, "None of the places the program's content can come from could be reached. It may be tried again."),
    (0x87D01201, "There isn't enough free space in the client cache, or the disk is full, to download the content."),
    (0x87D01202, "The content is bigger than the whole client cache, so it can't be downloaded until the cache is made larger."),
];
