//! Names of Component-Based Servicing (`CBS_E_`) and servicing stack
//! (`PSFX_E_`) result codes, which CBS, DISM and Windows Update logs report as
//! 0x800F08xx and 0x800F09xx. Windows has no message text for them, and the
//! Windows SDK doesn't define them, so a name is the best explanation there is.
//!
//! Each code and name was taken from a Microsoft Learn page that gives the two
//! together, in the same table row, and checked against the page a second
//! time. Only the names are kept, not Microsoft's descriptions:
//!
//! - <https://learn.microsoft.com/en-us/troubleshoot/azure/virtual-machines/windows/windows-update-errors-requiring-in-place-upgrade>
//! - <https://learn.microsoft.com/en-us/troubleshoot/windows-client/installing-updates-features-roles/common-windows-update-errors>
//!
//! Add a name only from a Microsoft source that gives its code with it.

/// Codes and their names, sorted by code.
pub(crate) const CBS_NAMES: &[(u32, &str)] = &[
    (0x800F0805, "CBS_E_INVALID_PACKAGE"),
    (0x800F080D, "CBS_E_MANIFEST_INVALID_ITEM"),
    (0x800F081F, "CBS_E_SOURCE_MISSING"),
    (0x800F0821, "CBS_E_ABORT"),
    (0x800F0825, "CBS_E_CANNOT_UNINSTALL"),
    (0x800F0830, "CBS_E_IMAGE_UNSERVICEABLE"),
    (0x800F0831, "CBS_E_STORE_CORRUPTION"),
    (0x800F0900, "CBS_E_XML_PARSER_FAILURE"),
    (0x800F0904, "CBS_E_MORE_THAN_ONE_ACTIVE_EDITION"),
    (0x800F0905, "CBS_E_NO_ACTIVE_EDITION"),
    (0x800F0911, "CBS_E_SOURCE_MODIFIED"),
    (0x800F0920, "CBS_E_HANG_DETECTED"),
    (0x800F0922, "CBS_E_INSTALLERS_FAILED"),
    (0x800F0982, "PSFX_E_MATCHING_COMPONENT_NOT_FOUND"),
    (0x800F0984, "PSFX_E_MATCHING_BINARY_MISSING"),
    (0x800F0985, "PSFX_E_APPLY_REVERSE_DELTA_FAILED"),
    (0x800F0986, "PSFX_E_APPLY_FORWARD_DELTA_FAILED"),
    (0x800F0987, "PSFX_E_NULL_DELTA_HYDRATION_FAILED"),
    (0x800F0988, "PSFX_E_INVALID_DELTA_COMBINATION"),
    (0x800F0989, "PSFX_E_REVERSE_DELTA_MISSING"),
    (0x800F0991, "PSFX_E_MISSING_PAYLOAD_FILE"),
];
