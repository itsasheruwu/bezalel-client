pub mod auth;
pub mod game_process;
pub mod launch;
pub mod official;

pub struct Profile {
    pub id: String,
    pub name: String,
    pub access_token: String,
    pub xuid: String,
    pub client_id: String,
}
