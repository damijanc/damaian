use std::ffi::c_void;

pub(crate) const KEYCHAIN_SERVICE: &str = "DamaianClient";
const KEYCHAIN_REF_PREFIX: &str = "keychain:";

pub(crate) fn reference_for_account(account: &str) -> Result<String, String> {
    validate_account(account)?;
    Ok(format!("{KEYCHAIN_REF_PREFIX}{}", account.trim()))
}

pub(crate) fn account_from_reference(reference: &str) -> Option<&str> {
    reference
        .strip_prefix(KEYCHAIN_REF_PREFIX)
        .map(str::trim)
        .filter(|account| !account.is_empty())
}

pub(crate) fn validate_account(account: &str) -> Result<(), String> {
    let account = account.trim();
    if account.is_empty() {
        return Err("Keychain account is required".to_string());
    }
    if account.chars().any(char::is_control) {
        return Err("Keychain account cannot contain control characters".to_string());
    }
    Ok(())
}

pub(crate) fn read_password(account: &str) -> Result<String, String> {
    validate_account(account)?;
    platform::read_password(KEYCHAIN_SERVICE, account.trim())
}

pub(crate) fn write_password(account: &str, password: &str) -> Result<(), String> {
    validate_account(account)?;
    if password.is_empty() {
        return Err("API key is required".to_string());
    }
    platform::write_password(KEYCHAIN_SERVICE, account.trim(), password)
}

pub(crate) fn delete_password(account: &str) -> Result<bool, String> {
    validate_account(account)?;
    platform::delete_password(KEYCHAIN_SERVICE, account.trim())
}

pub(crate) fn password_exists(account: &str) -> Result<bool, String> {
    validate_account(account)?;
    platform::password_exists(KEYCHAIN_SERVICE, account.trim())
}

#[cfg(target_os = "macos")]
mod platform {
    use super::c_void;
    use std::ptr;

    type OSStatus = i32;
    type UInt32 = u32;

    const ERR_SEC_SUCCESS: OSStatus = 0;
    const ERR_SEC_ITEM_NOT_FOUND: OSStatus = -25300;
    const ERR_SEC_DUPLICATE_ITEM: OSStatus = -25299;

    #[link(name = "Security", kind = "framework")]
    unsafe extern "C" {
        fn SecKeychainAddGenericPassword(
            keychain: *mut c_void,
            service_name_length: UInt32,
            service_name: *const i8,
            account_name_length: UInt32,
            account_name: *const i8,
            password_length: UInt32,
            password_data: *const c_void,
            item_ref: *mut *mut c_void,
        ) -> OSStatus;

        fn SecKeychainFindGenericPassword(
            keychain: *mut c_void,
            service_name_length: UInt32,
            service_name: *const i8,
            account_name_length: UInt32,
            account_name: *const i8,
            password_length: *mut UInt32,
            password_data: *mut *mut c_void,
            item_ref: *mut *mut c_void,
        ) -> OSStatus;

        fn SecKeychainItemModifyAttributesAndData(
            item_ref: *mut c_void,
            attr_list: *mut c_void,
            length: UInt32,
            data: *const c_void,
        ) -> OSStatus;

        fn SecKeychainItemDelete(item_ref: *mut c_void) -> OSStatus;

        fn SecKeychainItemFreeContent(attr_list: *mut c_void, data: *mut c_void) -> OSStatus;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(cf: *const c_void);
    }

    pub(super) fn read_password(service: &str, account: &str) -> Result<String, String> {
        let mut password_length: UInt32 = 0;
        let mut password_data: *mut c_void = ptr::null_mut();
        let mut item_ref: *mut c_void = ptr::null_mut();
        let status = unsafe {
            SecKeychainFindGenericPassword(
                ptr::null_mut(),
                service.len() as UInt32,
                service.as_ptr() as *const i8,
                account.len() as UInt32,
                account.as_ptr() as *const i8,
                &mut password_length,
                &mut password_data,
                &mut item_ref,
            )
        };
        if status == ERR_SEC_ITEM_NOT_FOUND {
            return Err(format!("No Keychain item found for account '{account}'"));
        }
        check_status(status, "read Keychain item")?;

        let password = unsafe {
            let bytes =
                std::slice::from_raw_parts(password_data as *const u8, password_length as usize);
            String::from_utf8(bytes.to_vec())
                .map_err(|_| "Keychain item is not valid UTF-8".to_string())?
        };
        unsafe {
            SecKeychainItemFreeContent(ptr::null_mut(), password_data);
            if !item_ref.is_null() {
                CFRelease(item_ref);
            }
        }
        Ok(password)
    }

    pub(super) fn write_password(
        service: &str,
        account: &str,
        password: &str,
    ) -> Result<(), String> {
        let mut item_ref: *mut c_void = ptr::null_mut();
        let find_status = unsafe {
            SecKeychainFindGenericPassword(
                ptr::null_mut(),
                service.len() as UInt32,
                service.as_ptr() as *const i8,
                account.len() as UInt32,
                account.as_ptr() as *const i8,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut item_ref,
            )
        };

        if find_status == ERR_SEC_SUCCESS {
            let status = unsafe {
                SecKeychainItemModifyAttributesAndData(
                    item_ref,
                    ptr::null_mut(),
                    password.len() as UInt32,
                    password.as_ptr() as *const c_void,
                )
            };
            unsafe {
                if !item_ref.is_null() {
                    CFRelease(item_ref);
                }
            }
            return check_status(status, "update Keychain item");
        }
        if find_status != ERR_SEC_ITEM_NOT_FOUND {
            return check_status(find_status, "find Keychain item");
        }

        let status = unsafe {
            SecKeychainAddGenericPassword(
                ptr::null_mut(),
                service.len() as UInt32,
                service.as_ptr() as *const i8,
                account.len() as UInt32,
                account.as_ptr() as *const i8,
                password.len() as UInt32,
                password.as_ptr() as *const c_void,
                ptr::null_mut(),
            )
        };
        if status == ERR_SEC_DUPLICATE_ITEM {
            return write_password(service, account, password);
        }
        check_status(status, "add Keychain item")
    }

    pub(super) fn delete_password(service: &str, account: &str) -> Result<bool, String> {
        let mut item_ref: *mut c_void = ptr::null_mut();
        let status = unsafe {
            SecKeychainFindGenericPassword(
                ptr::null_mut(),
                service.len() as UInt32,
                service.as_ptr() as *const i8,
                account.len() as UInt32,
                account.as_ptr() as *const i8,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut item_ref,
            )
        };
        if status == ERR_SEC_ITEM_NOT_FOUND {
            return Ok(false);
        }
        check_status(status, "find Keychain item")?;
        let delete_status = unsafe { SecKeychainItemDelete(item_ref) };
        unsafe {
            if !item_ref.is_null() {
                CFRelease(item_ref);
            }
        }
        check_status(delete_status, "delete Keychain item")?;
        Ok(true)
    }

    pub(super) fn password_exists(service: &str, account: &str) -> Result<bool, String> {
        let mut item_ref: *mut c_void = ptr::null_mut();
        let status = unsafe {
            SecKeychainFindGenericPassword(
                ptr::null_mut(),
                service.len() as UInt32,
                service.as_ptr() as *const i8,
                account.len() as UInt32,
                account.as_ptr() as *const i8,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut item_ref,
            )
        };
        if status == ERR_SEC_ITEM_NOT_FOUND {
            return Ok(false);
        }
        check_status(status, "find Keychain item")?;
        unsafe {
            if !item_ref.is_null() {
                CFRelease(item_ref);
            }
        }
        Ok(true)
    }

    fn check_status(status: OSStatus, action: &str) -> Result<(), String> {
        if status == ERR_SEC_SUCCESS {
            Ok(())
        } else {
            Err(format!("Failed to {action}: OSStatus {status}"))
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    pub(super) fn read_password(_service: &str, _account: &str) -> Result<String, String> {
        Err("macOS Keychain is only available on macOS".to_string())
    }

    pub(super) fn write_password(
        _service: &str,
        _account: &str,
        _password: &str,
    ) -> Result<(), String> {
        Err("macOS Keychain is only available on macOS".to_string())
    }

    pub(super) fn delete_password(_service: &str, _account: &str) -> Result<bool, String> {
        Err("macOS Keychain is only available on macOS".to_string())
    }

    pub(super) fn password_exists(_service: &str, _account: &str) -> Result<bool, String> {
        Err("macOS Keychain is only available on macOS".to_string())
    }
}
