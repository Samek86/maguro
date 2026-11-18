pub(crate) mod ctxlog;
pub(crate) mod openlog;
pub(crate) mod fileopen;
pub(crate) mod ctxmenu;
pub(crate) mod ctxpack;
mod excelro;
pub(crate) mod blockext;
pub(crate) mod warmup;
mod askdrop;
mod tabdrop;
mod fastcopy;
mod fav;
mod itemopen;
mod termadmin;
mod fileop;
pub(crate) mod listsync;
mod gather;
pub(crate) mod inuse;
mod pins;

pub(crate) use askdrop::*;
pub(crate) use tabdrop::*;
pub(crate) use fav::*;
pub(crate) use itemopen::*;
pub(crate) use fastcopy::apply_fast_note;
pub(crate) use fileop::*;
pub(crate) use termadmin::launch as launch_admin_terminal;
pub(crate) use termadmin::open_here;

pub(crate) fn terminal_dir(shown: &str) -> Option<String> {
    termadmin::terminal_folder(true, shown, &[])
}
pub(crate) use gather::*;
pub(crate) use pins::*;
