use num_traits::*;
use ux_api::menu::*;

use crate::VaultOp;

pub fn create_submenu(vault_conn: xous::CID, _actions_conn: xous::CID, menu_mgr: xous::SID) -> MenuMatic {
    let mut menu_items = Vec::<MenuItem>::new();

    menu_items.push(MenuItem {
        name: String::from("Share"),
        action_conn: Some(vault_conn),
        action_opcode: VaultOp::BaogramShareOp.to_u32().unwrap(),
        action_payload: MenuPayload::Scalar([0, 0, 0, 0]),
        close_on_select: true,
    });
    menu_items.push(MenuItem {
        name: String::from("Delete"),
        action_conn: Some(vault_conn),
        action_opcode: VaultOp::BaogramDeleteOp.to_u32().unwrap(),
        action_payload: MenuPayload::Scalar([0, 0, 0, 0]),
        close_on_select: true,
    });
    menu_items.push(MenuItem {
        name: String::from("Author"),
        action_conn: Some(vault_conn),
        action_opcode: VaultOp::BaogramAuthorOp.to_u32().unwrap(),
        action_payload: MenuPayload::Scalar([0, 0, 0, 0]),
        close_on_select: true,
    });
    menu_items.push(MenuItem {
        name: String::from("Back"),
        action_conn: Some(vault_conn),
        action_opcode: VaultOp::BaogramBackOp.to_u32().unwrap(),
        action_payload: MenuPayload::Scalar([0, 0, 0, 0]),
        close_on_select: true,
    });
    menu_items.push(MenuItem {
        name: String::from("Exit Baogram"),
        action_conn: Some(vault_conn),
        action_opcode: VaultOp::BaogramExitOp.to_u32().unwrap(),
        action_payload: MenuPayload::Scalar([0, 0, 0, 0]),
        close_on_select: true,
    });

    menu_matic(menu_items, "Baogram", Some(menu_mgr), vault_conn, VaultOp::MenuDone.to_usize().unwrap())
        .expect("couldn't create MenuMatic manager")
}
