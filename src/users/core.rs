/// users -- print the user names of users currently logged in
///
/// Reads utmpx records and prints a sorted, space-separated list of login names
/// for all USER_PROCESS entries.

use std::ffi::CStr;

/// Retrieve a sorted list of currently logged-in user names from utmpx.
///
/// # Safety
/// Uses libc's setutxent/getutxent/endutxent which are not thread-safe.
/// This function must not be called concurrently.
pub fn get_users() -> Vec<String> {
    let mut users = Vec::new();

    // SAFETY: setutxent/getutxent/endutxent are standard POSIX functions.
    // We call them sequentially and do not hold pointers across calls.
    unsafe {
        libc::setutxent();
        loop {
            let entry = libc::getutxent();
            if entry.is_null() {
                break;
            }
            let entry = &*entry;
            if entry.ut_type == libc::USER_PROCESS {
                let name = CStr::from_ptr(entry.ut_user.as_ptr())
                    .to_string_lossy()
                    .to_string();
                if !name.is_empty() {
                    users.push(name);
                }
            }
        }
        libc::endutxent();
    }

    users.sort();
    users
}

/// Format the user list as a single space-separated line (matching GNU users output).
pub fn format_users(users: &[String]) -> String {
    users.join(" ")
}
