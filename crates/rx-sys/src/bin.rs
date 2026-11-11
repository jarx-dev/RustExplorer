//! The Recycle Bin for Ctrl+Z: what a delete sent there is found by its original folder, name and
//! deletion time, and put back with the bin's own "Przywróć"; no file inside $Recycle.Bin is touched.
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use windows::core::{GUID, PCSTR, PWSTR};
use windows::Win32::Foundation::{FILETIME, PROPERTYKEY};
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, CoUninitialize, IBindCtx, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::{CreatePopupMenu, DestroyMenu, SW_SHOWNORMAL};

// propkey.h: System.Recycle.DeletedFrom and System.Recycle.DateDeleted (FMTID_Displaced)
const DISPLACED: GUID = GUID::from_u128(0x9b174b33_40ff_11d2_a27e_00c04fc30871);
const FROM: PROPERTYKEY = PROPERTYKEY { fmtid: DISPLACED, pid: 2 };
const DELETED: PROPERTYKEY = PROPERTYKEY { fmtid: DISPLACED, pid: 3 };

fn time(ft: FILETIME) -> SystemTime {
    // FILETIME counts 100 ns from 1601
    let t = ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64;
    SystemTime::UNIX_EPOCH + Duration::from_nanos(t.saturating_sub(116_444_736_000_000_000) * 100)
}

fn take(p: PWSTR) -> String {
    let s = unsafe { p.to_string() }.unwrap_or_default();
    unsafe { CoTaskMemFree(Some(p.0 as _)) };
    s
}

/// Each bin item: (original folder, name as the bin shows it, deletion time, the item, its own file
/// in the bin, read only to compare extensions).
type Item = (String, String, SystemTime, IShellItem, String);

fn items() -> windows::core::Result<Vec<Item>> {
    let mut out = vec![];
    unsafe {
        let bin: IShellItem = SHGetKnownFolderItem(&FOLDERID_RecycleBinFolder, KF_FLAG_DEFAULT, None)?;
        let all: IEnumShellItems = bin.BindToHandler(None::<&IBindCtx>, &BHID_EnumItems)?;
        loop {
            let mut one = [None];
            let mut n = 0;
            if all.Next(&mut one, Some(&mut n)).is_err() || n == 0 {
                break;
            }
            let Some(item) = one[0].take() else { break };
            let Ok(i2) = windows::core::Interface::cast::<IShellItem2>(&item) else { continue };
            let Ok(at) = i2.GetFileTime(&DELETED) else { continue };
            let from = i2.GetString(&FROM).map(take).unwrap_or_default();
            let name = item.GetDisplayName(SIGDN_PARENTRELATIVEEDITING).map(take).unwrap_or_default();
            let file = item.GetDisplayName(SIGDN_FILESYSPATH).map(take).unwrap_or_default();
            out.push((from, name, time(at), item, file));
        }
    }
    Ok(out)
}

/// Puts `paths`, sent to the Recycle Bin between `since` and `until`, back where they were. Returns
/// those it could not, with the reason: the place is taken (nothing is overwritten), or the item is
/// not in the bin (emptied, or deleted for good). Runs on a thread of its own: the bin's commands
/// need a single-threaded COM apartment.
pub fn restore(paths: &[PathBuf], since: SystemTime, until: SystemTime) -> Result<Vec<(PathBuf, &'static str)>, String> {
    let paths = paths.to_vec();
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let r = restore_here(&paths, since, until).map_err(|e| e.message());
        CoUninitialize();
        r
    })
    .join()
    .unwrap_or_else(|_| Err(rx_core::t!("rx_sys.internal").into()))
}

fn restore_here(paths: &[PathBuf], since: SystemTime, until: SystemTime) -> windows::core::Result<Vec<(PathBuf, &'static str)>> {
    let mut failed = vec![];
    let mut want = vec![];
    for p in paths {
        if p.symlink_metadata().is_ok() {
            failed.push((p.clone(), rx_core::t!("rx_sys.place_taken")));
        } else {
            want.push(p);
        }
    }
    if want.is_empty() {
        return Ok(failed);
    }
    let all = items()?;
    // for each path the match of its own extension first, then the newest: the same name may have
    // been deleted there before
    let rank = |p: &PathBuf, i: usize| (rx_core::undo::bin_same_ext(p, &all[i].4), all[i].2);
    let mut found: Vec<Option<usize>> = vec![None; want.len()];
    for (i, (from, name, at, ..)) in all.iter().enumerate() {
        for (k, p) in want.iter().enumerate() {
            if rx_core::undo::bin_match(p, from, name, *at, since, until) && found[k].is_none_or(|j| rank(p, j) < rank(p, i)) {
                found[k] = Some(i);
            }
        }
    }
    let mut picked: Vec<usize> = found.iter().flatten().copied().collect();
    picked.sort_unstable();
    picked.dedup();
    if !picked.is_empty() {
        undelete(picked.iter().map(|&i| &all[i].3))?;
    }
    for (k, p) in want.into_iter().enumerate() {
        if found[k].is_none() {
            failed.push((p.clone(), rx_core::t!("rx_sys.not_in_bin")));
        } else if p.symlink_metadata().is_err() {
            failed.push((p.clone(), rx_core::t!("rx_sys.not_restored")));
        }
    }
    Ok(failed)
}

/// The bin's "Przywróć" for these items of it, as Explorer runs it.
fn undelete<'a>(items: impl Iterator<Item = &'a IShellItem>) -> windows::core::Result<()> {
    unsafe {
        let mut pidls: Vec<*mut ITEMIDLIST> = vec![];
        let made = (|| {
            for it in items {
                pidls.push(SHGetIDListFromObject(it)?);
            }
            let list: Vec<*const ITEMIDLIST> = pidls.iter().map(|&p| p as *const _).collect();
            let array = SHCreateShellItemArrayFromIDLists(&list)?;
            let menu: IContextMenu = array.BindToHandler(None::<&IBindCtx>, &BHID_SFUIObject)?;
            let hmenu = CreatePopupMenu()?;
            let r = menu.QueryContextMenu(hmenu, 0, 1, 0x7FFF, CMF_NORMAL).ok().and_then(|_| {
                // no "no UI" flag: should a place be taken meanwhile, the shell asks rather than replaces
                let info = CMINVOKECOMMANDINFO {
                    cbSize: std::mem::size_of::<CMINVOKECOMMANDINFO>() as u32,
                    lpVerb: PCSTR(c"undelete".as_ptr() as *const u8),
                    nShow: SW_SHOWNORMAL.0,
                    ..Default::default()
                };
                menu.InvokeCommand(&info)
            });
            let _ = DestroyMenu(hmenu);
            r
        })();
        for p in pidls {
            CoTaskMemFree(Some(p as _));
        }
        made
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_recycled_file_comes_back_from_the_bin() {
        // next to the test EXE: the test folder, not the user's own
        let dir = std::env::current_exe().unwrap().parent().unwrap().to_path_buf();
        let p = dir.join(format!("rx-test-bin-{}.txt", std::process::id()));
        std::fs::write(&p, "rx").unwrap();
        crate::init_com();
        let since = std::time::SystemTime::now();
        assert!(crate::recycle(std::slice::from_ref(&p)));
        let until = std::time::SystemTime::now();
        assert!(!p.exists(), "not deleted");
        for (from, name, at, _, file) in super::items().unwrap().iter().filter(|x| x.0.eq_ignore_ascii_case(&dir.to_string_lossy())) {
            eprintln!("bin: {from} | {name} | {file} | {at:?}");
        }
        let failed = super::restore(std::slice::from_ref(&p), since, until).unwrap();
        assert!(failed.is_empty(), "{failed:?}");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "rx");
        // a second restore finds the place taken and touches nothing
        let again = super::restore(std::slice::from_ref(&p), since, until).unwrap();
        assert_eq!(again.len(), 1);
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn a_copy_in_its_own_folder_reports_the_name_it_got() {
        let dir = std::env::current_exe().unwrap().parent().unwrap().to_path_buf();
        let p = dir.join(format!("rx-test-copy-{}.txt", std::process::id()));
        std::fs::write(&p, "rx").unwrap();
        crate::init_com();
        let (ok, undo, left) = crate::copy_into(std::slice::from_ref(&p), &dir, false);
        eprintln!("copy: {ok} {undo:?} {left}");
        let rx_core::undo::Undo::Created(made, _) = undo else { panic!() };
        let _ = std::fs::remove_file(&p);
        for (m, _) in &made {
            let _ = std::fs::remove_file(m);
        }
        assert!(ok && left == 0 && made.len() == 1 && made[0].0 != p, "{made:?}");
    }

    #[test]
    fn a_move_goes_back_and_never_onto_a_taken_place() {
        let dir = std::env::current_exe().unwrap().parent().unwrap().join(format!("rx-test-move-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("in")).unwrap();
        let p = dir.join("a.txt");
        std::fs::write(&p, "rx").unwrap();
        crate::init_com();
        let (ok, undo, _) = crate::copy_into(std::slice::from_ref(&p), &dir.join("in"), true);
        let rx_core::undo::Undo::Moved(items) = undo else { panic!() };
        assert!(ok && items == [(dir.join("in").join("a.txt"), p.clone())], "{items:?}");
        // the place taken meanwhile: left as it is
        std::fs::write(&p, "new").unwrap();
        assert_eq!(crate::move_back(&items).len(), 1);
        std::fs::remove_file(&p).unwrap();
        assert!(crate::move_back(&items).is_empty());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "rx");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
