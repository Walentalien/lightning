use std::collections::HashMap;
use std::path::PathBuf;
use anyhow::Result;
use config::Value;
use lightning_guard::map::{FileRule, PacketFilterRule, Profile};
use lightning_guard::ConfigSource;
use log::error;

use reqwest::Client;
use serde::Deserialize;
pub struct State {
    filters: Vec<PacketFilterRule>,
    profiles: HashMap<Option<PathBuf>, Profile>,
    selected_profile: Option<PathBuf>,
    src: ConfigSource,
    current_epoch: Option<u64>,
    ownership_info: OwnershipInfo,
    participation: String,
    reputation: String,
    uptime: String,
    stake_info: StakeInfo,
    committee_members: Vec<String>,
}
struct OwnershipInfo {
    owner_address: String,
    public_keys: PublicKeys,
}

//TODO: Can be optimized using serde::Value
#[derive(Deserialize)]
struct Response<T> {          //TODO Unify
    jsonrpc: String,
    result: T,
    id: u64,
}
#[derive(Debug, Deserialize)]
pub struct ApiResponse<T> {
    pub jsonrpc: String,
    pub result: Vec<ResultField<T>>,
    pub id: u64,
}
#[derive(Debug, Deserialize)]
pub struct ApiResponseKeys<T>{
    pub jsonrpc: String,
    pub result: T,
    pub id: u64,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum ResultField<T> {
    NodeInfo(T),
    Number(u64),
}
#[derive(Debug, Deserialize)]
struct PublicKeys{
   node_public_key: String,
   consensus_public_key:String,
}

#[derive(Debug, Deserialize)]
pub struct NodeInfo {
    pub owner: String,
    pub public_key: String,
    pub consensus_key: String,
    pub staked_since: u64,
    pub stake: StakeInfo,
    pub domain: String,
    pub worker_domain: String,
    pub ports: Ports,
    pub worker_public_key: String,
    pub participation: String, // Change to bool if it can be deserialized as true/false instead of a string
    pub nonce: u64,
}


// Stake information structure
#[derive(Debug, Deserialize)]
pub struct StakeInfo {
    pub staked: String,
    pub stake_locked_until: u64,
    pub locked: String,
    pub locked_until: u64,
}

// Port information structure
#[derive(Debug, Deserialize)]
pub struct Ports {
    pub primary: u16,
    pub worker: u16,
    pub mempool: u16,
    pub rpc: u16,
    pub pool: u16,
    pub pinger: u16,
    pub handshake: HandshakePorts,
}

// Handshake port structure
#[derive(Debug, Deserialize)]
pub struct HandshakePorts {
    pub http: u16,
    pub webrtc: u16,
    pub webtransport: u16,
}
impl State {
    pub fn new(src: ConfigSource) -> Self {
        Self {
            filters: Vec::new(),
            profiles: HashMap::new(),
            selected_profile: None,
            src,
            current_epoch: None,
            ownership_info: OwnershipInfo {
                owner_address: "".to_string(),
                public_keys: PublicKeys {
                    node_public_key: "".to_string(),
                    consensus_public_key: "".to_string(),
                }
            },
            participation: "".to_string(),
            reputation: "".to_string(),
            uptime: "".to_string(),
            stake_info: StakeInfo {
                staked: "".to_string(),
                stake_locked_until: 0,
                locked: "".to_string(),
                locked_until: 0,
            },
            committee_members: Vec::new(),



        }
    }

    pub async fn load_filters(&mut self) -> Result<()> {
        self.filters = self.src.read_packet_filters().await?;
        Ok(())
    }

    pub fn add_filters(&mut self, filters: Vec<PacketFilterRule>) {
        self.filters = filters;
    }

    pub fn add_filter(&mut self, filter: PacketFilterRule) {
        self.filters.push(filter);
    }

    pub fn commit_filters(&mut self) {
        let filters = self.filters.clone();
        let src = self.src.clone();
        tokio::spawn(async move {
            if let Err(e) = src.write_packet_filters(filters).await {
                error!("failed to write profiles to disk: {e:?}");
            }
        });
    }

    pub fn get_filters(&self) -> &[PacketFilterRule] {
        self.filters.as_slice()
    }

    pub async fn load_profiles(&mut self) -> Result<()> {
        self.profiles = self
            .src
            .get_profiles()
            .await?
            .into_iter()
            .map(|p| (p.name.clone(), p))
            .collect();
        Ok(())
    }




    pub async fn write_current_epoch(&mut self) -> Result<()> {
        // Define the endpoint URL
        let url = "http://104.131.168.39:4230/rpc/v0";

        // Define the JSON payload
        let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "flk_get_epoch",
        "params": [],
        "id": 1
    });

        // Create an HTTP client
        let client = Client::new();

        // Send the POST request
        let response = client
            .post(url)
            .json(&payload)
            .send()
            .await?;

        // Parse the JSON response
        let response_json: Response<u64> = response.json().await?;

        // Extract the epoch value
        self.current_epoch = Some(response_json.result);

       Ok(())
    }

    pub async fn write_current_network_info(&mut self) -> Result<()> {

        let url = "http://104.131.168.39:4230/rpc/v0";
        //let url = "http://fleek-test.network:4240/rpc/v0";
        let client = reqwest::Client::new();
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "flk_get_public_keys",
            "params": [],
            "id": 1, // TODO: Implement requestID logic

        });

        let response = client
            .post(url)
            .json(&payload)
            .send()
            .await?;

        let response_json: ApiResponseKeys<PublicKeys> = response.json().await?;
        let public_key : String = response_json.result.node_public_key.clone();
        let consensus_key : String = response_json.result.consensus_public_key.clone();
        self.ownership_info.public_keys.node_public_key = public_key.clone();
        self.ownership_info.public_keys.consensus_public_key = consensus_key.clone();
        let client = reqwest::Client::new();

        let client = Client::new();
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "flk_get_reputation",
            "params": [public_key],
            "id": 2, // TODO: Implement requestID logic

        });
        let response = client.post(url).json(&payload).send().await?;

        let response_json :Response<Option<String>> = response.json().await?;
        let reputation:Option<String> = Some(response_json.result.expect("Getting uptime failed"));
        match reputation {
            Some(reputation) => {
                self.reputation = reputation;
            }
            None => {
                //self.reputation = "No reputation available".to_string();
                self.reputation = "0".to_string();
            }
        }

        let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "flk_get_node_uptime",
        "params": [public_key],
        "id": 3,
        });
        let response = client.post(url).json(&payload).send().await?;
        let response_json:Response<Option<String>> = response.json().await?;
        self.uptime = response_json.result.expect("Retrieving uptime failed").to_string();

        // match uptime {
        //     Some(uptime) => {
        //         self.uptime = uptime;
        //     }
        //     None => {
        //         //self.reputation = "No reputation available".to_string();
        //         self.uptime = "0".to_string();
        //     }
        // }



        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "flk_get_node_info_epoch",
            "params": [public_key],
            "id": 2,
        });

        let response = client.post(url).json(&payload).send().await?;

        let api_response: ApiResponse<NodeInfo> = response.json().await?;

        for result in api_response.result {
            match result{
                ResultField::NodeInfo(info) => {
                    self.ownership_info.owner_address = info.owner;
                    self.ownership_info.public_keys.node_public_key = info.public_key;
                    self.ownership_info.public_keys.consensus_public_key = info.consensus_key;
                    self.stake_info.staked = info.stake.staked;
                    self.stake_info.stake_locked_until = info.stake.stake_locked_until;
                    self.stake_info.locked = info.stake.locked;
                    self.stake_info.locked_until = info.stake.locked_until;
                    self.participation = info.participation;

                }
                ResultField::Number(number) => continue,
            }
        }

        // writing committee members to the struct
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "flk_get_committee_members",
            "params": [],
            "id": 8,
        });
        let response = client.post(url).json(&payload).send().await?;
        //let response_json:Response<Value> = response.json().await?;
        // if let Some(committee_members) = response_json["result"].as_array() {
        //     f
        // }
        Ok(())

    }

    pub fn get_epoch(&self) -> u64 {
        self.current_epoch.unwrap_or(0)
    }

    pub fn get_ethereum_address(&self) -> String {
        self.ownership_info.owner_address.clone()
    }
    pub fn get_node_public_key(&self) -> String {
        self.ownership_info.public_keys.node_public_key.clone()
    }

    pub fn get_consensus_public_key(&self) -> String {
        self.ownership_info.public_keys.consensus_public_key.clone()
    }

    pub fn get_staked(&self) -> String {
        self.stake_info.staked.clone()
    }
    pub fn get_stake_locked_until(&self) -> u64 {
        self.stake_info.stake_locked_until
    }

    pub fn get_locked(&self) -> String {
        self.stake_info.locked.clone()
    }
    pub fn get_locked_until(&self) -> u64 {
        self.stake_info.locked_until
    }

    pub fn get_participation(&self) -> String {
        self.participation.clone()
    }

    pub fn get_reputation(&self) -> String {
        self.reputation.clone()
    }

    pub fn get_uptime(&self) -> String {
        self.uptime.clone()
    }

    pub fn get_committee_members(&self) -> Vec<String> {
        self.committee_members.clone()
    }


    pub fn add_profile(&mut self, profiles: Profile) {
        self.profiles.insert(profiles.name.clone(), profiles);
    }

    pub fn commit_add_profiles(&mut self) {
        let profiles = self.profiles.clone().into_iter().map(|(_, p)| p).collect();
        let src = self.src.clone();
        tokio::spawn(async move {
            if let Err(e) = src.write_profiles(profiles).await {
                error!("failed to write profiles to disk: {e:?}");
            }
        });
    }

    pub fn commit_remove_profiles(&mut self, remove: Vec<Option<PathBuf>>) {
        let src = self.src.clone();
        tokio::spawn(async move {
            if let Err(e) = src.delete_profiles(remove.into_iter().collect()).await {
                error!("failed to write profiles to disk: {e:?}");
            }
        });
    }

    pub fn get_profiles(&self) -> Vec<Profile> {
        self.profiles.values().cloned().collect()
    }

    pub fn update_selected_profile_rules_list(&mut self, rules: Vec<FileRule>) {
        let name = &self.selected_profile;
        let profile = self.profiles.get_mut(name).expect("Profile to exist");
        profile.file_rules = rules;
    }

    pub fn get_profile_rules(&self, name: &Option<PathBuf>) -> &[FileRule] {
        self.profiles
            .get(name)
            .expect("There to be a profile")
            .file_rules
            .as_slice()
    }

    pub fn get_selected_profile(&self) -> Option<&Profile> {
        self.profiles.get(&self.selected_profile)
    }

    pub fn get_selected_profile_mut(&mut self) -> Option<&mut Profile> {
        self.profiles.get_mut(&self.selected_profile)
    }

    pub fn select_profile(&mut self, profile: &Profile) {
        self.selected_profile = profile.name.clone();
        debug_assert!(self.profiles.contains_key(&self.selected_profile));
    }

    pub fn update_profiles(&mut self, profiles: Vec<Profile>) {
        self.profiles = profiles.into_iter().map(|p| (p.name.clone(), p)).collect();
    }
}
