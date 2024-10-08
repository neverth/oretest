use std::{sync::Arc, time::Instant};
use std::error::Error;
use std::time::Duration;
use colored::*;
use drillx::{
    equix::{self},
    Hash, Solution,
};
use futures_util::future::join_all;
use ore_api::{
    consts::{BUS_ADDRESSES, BUS_COUNT, EPOCH_DURATION},
    state::{Config, Proof},
};
use rand::Rng;
use serde::{Deserialize, Serialize};
use solana_program::pubkey::Pubkey;
use solana_rpc_client::spinner;
use solana_sdk::signer::Signer;
use tokio::sync::RwLock;
use tokio::time::timeout;
use crate::{
    args::MineArgs,
    send_and_confirm::ComputeBudget,
    utils::{amount_u64_to_string, get_clock, get_config, get_proof_with_authority, proof_pubkey},
    Miner,
};
use crate::jito_send_and_confirm::{JitoTips, subscribe_jito_tips};


async fn fetch_data(client: &reqwest::Client, url: &str, t: u64) -> Result<Response, String> {
    println!("req {}", url);
    let timeout_duration = Duration::from_secs(t);
    match timeout(timeout_duration, client.get(url).send()).await {
        Ok(response) => match response {
            Ok(resp) => {
                tokio::spawn(client.get("http://154.9.28.82:8090/metric?name=ore_fetch_data&type=counter&method=add&value=1&tags=[\"is_succ\"]&tag_values=[\"true\"]").send());
                resp.json::<Response>().await.map_err(|e| e.to_string())
            }
            Err(e) => {
                Err(e.to_string())
            }
        },
        Err(_) => Err("Timeout error".to_string()),
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct Response {
    d: [u8; 16],
    n: [u8; 8],
    challenge: String,
    best_difficulty: u64,
}


impl Miner {
    pub async fn mine(&self, args: MineArgs) {
        // Register, if needed.
        let signer = self.signer();
        self.open().await;

        // Check num threads
        self.check_num_cores(args.threads);

        let tips = Arc::new(RwLock::new(JitoTips::default()));
        subscribe_jito_tips(tips.clone()).await;

        let mut last_reward: u64 = 0;
        let mut last_use: u64 = 0;

        // Start mining loop
        loop {
            // Fetch proof
            let proof = get_proof_with_authority(&self.rpc_client, signer.pubkey()).await;
            println!(
                "\nStake balance: {} ORE, reward diff {}, fee {}, diff {}",
                amount_u64_to_string(proof.balance),
                amount_u64_to_string(proof.balance - last_reward),
                amount_u64_to_string(last_use),
                amount_u64_to_string(proof.balance - last_reward - last_use),
            );

            last_reward = proof.balance;

            // Calc cutoff time
            let cutoff_time = self.get_cutoff(proof, args.buffer_time).await;

            // Run drillx
            let config = get_config(&self.rpc_client).await;

            let client = reqwest::Client::new();

            let results = join_all(vec![
                // fetch_data(&client, &format!(
                //     "http://192.168.31.155:6789/ore?cutoff_time={}&threads={}&min_difficulty={}&challenge={:?}&total_div={}&start_idx={}",
                //     cutoff_time, 16, config.min_difficulty, proof.challenge, 60, 0),
                // ),
                fetch_data(&client, &format!(
                    "http://127.0.0.1:6789/ore?cutoff_time={}&threads={}&min_difficulty={}&challenge={:?}&total_div={}&start_idx={}",
                    cutoff_time, 16, config.min_difficulty, proof.challenge, 44, 0), cutoff_time + 15,
                ),
                fetch_data(&client, &format!(
                    "http://192.168.31.178:6789/ore?cutoff_time={}&threads={}&min_difficulty={}&challenge={:?}&total_div={}&start_idx={}",
                    cutoff_time, 12, config.min_difficulty, proof.challenge, 44, 16), cutoff_time + 15,
                ),
                fetch_data(&client, &format!(
                    "http://45.159.228.77:6789/ore?cutoff_time={}&threads={}&min_difficulty={}&challenge={:?}&total_div={}&start_idx={}",
                    cutoff_time, 4, config.min_difficulty, proof.challenge, 44, 32), cutoff_time + 15,
                ),
                fetch_data(&client, &format!(
                    "http://45.159.229.105:6789/ore?cutoff_time={}&threads={}&min_difficulty={}&challenge={:?}&total_div={}&start_idx={}",
                    cutoff_time, 4, config.min_difficulty, proof.challenge, 44, 36), cutoff_time + 15,
                ),
            ])
                .await;

            let mut best_response = Response {
                d: [0u8; 16],
                n: [0u8; 8],
                challenge: String::new(),
                best_difficulty: 0,
            };

            for (i, result) in results.iter().enumerate() {
                match result {
                    Ok(response) => {
                        println!("Result {}: {:?}", i + 1, response);

                        tokio::spawn(client.get(format!("http://154.9.28.82:8090/metric?name=ore_diffculty&type=gauge&method=set&value={}&tags=[\"id\"]&tag_values=[\"{}\"]", response.best_difficulty, i)).send());

                        if response.best_difficulty >= best_response.best_difficulty {
                            best_response.d = response.d;
                            best_response.n = response.n;
                            best_response.best_difficulty = response.best_difficulty;
                        }
                    }
                    Err(e) => println!("Error in Result {}: {}", i + 1, e),
                }
            }

            tokio::spawn(client.get(format!("http://154.9.28.82:8090/metric?name=ore_diffculty&type=gauge&method=set&value={}&tags=[\"id\"]&tag_values=[\"{}\"]", best_response.best_difficulty, 999)).send());

            println!("best_response {:?}", best_response);

            let solution = Solution::new(best_response.d, best_response.n);

            //
            // let solution = Self::find_hash_par(
            //     proof,
            //     cutoff_time,
            //     args.threads,
            //     config.min_difficulty as u32,
            // )
            //     .await;

            // Submit most difficult hash
            let mut compute_budget = 500_000;
            let mut ixs = vec![ore_api::instruction::auth(proof_pubkey(signer.pubkey()))];
            // if self.should_reset(config).await {
            //     compute_budget += 100_000;
            //     ixs.push(ore_api::instruction::reset(signer.pubkey()));
            // }
            ixs.push(ore_api::instruction::mine(
                signer.pubkey(),
                signer.pubkey(),
                find_bus(),
                solution,
            ));
            let confirm_resp = self.jito_send_and_confirm(&ixs, ComputeBudget::Fixed(compute_budget), false, tips.clone(), best_response.best_difficulty)
                .await;

            println!("{:?}", confirm_resp);

            match confirm_resp {
                Ok(value) => {
                    last_use = ((value * 10000000000) as f64 * 1.75) as u64;
                    tokio::spawn(client.get("http://154.9.28.82:8090/metric?name=ore_fee_counter&type=counter&method=add&value=1&tags=[\"is_succ\"]&tag_values=[\"true\"]").send());
                }
                Err(_) => {
                    tokio::spawn(client.get("http://154.9.28.82:8090/metric?name=ore_fee_counter&type=counter&method=add&value=1&tags=[\"is_succ\"]&tag_values=[\"false\"]").send());
                }
            }
        }
    }

    async fn find_hash_par(
        proof: Proof,
        cutoff_time: u64,
        threads: u64,
        min_difficulty: u32,
    ) -> Solution {
        // Dispatch job to each thread
        let progress_bar = Arc::new(spinner::new_progress_bar());
        progress_bar.set_message("Mining...");
        let handles: Vec<_> = (0..threads)
            .map(|i| {
                std::thread::spawn({
                    let proof = proof.clone();
                    let progress_bar = progress_bar.clone();
                    let mut memory = equix::SolverMemory::new();
                    move || {
                        let timer = Instant::now();
                        let mut nonce = u64::MAX.saturating_div(threads).saturating_mul(i);
                        let mut best_nonce = nonce;
                        let mut best_difficulty = 0;
                        let mut best_hash = Hash::default();
                        loop {
                            // Create hash
                            if let Ok(hx) = drillx::hash_with_memory(
                                &mut memory,
                                &proof.challenge,
                                &nonce.to_le_bytes(),
                            ) {
                                let difficulty = hx.difficulty();
                                if difficulty.gt(&best_difficulty) {
                                    best_nonce = nonce;
                                    best_difficulty = difficulty;
                                    best_hash = hx;
                                }
                            }

                            // Exit if time has elapsed
                            if nonce % 100 == 0 {
                                if timer.elapsed().as_secs().ge(&cutoff_time) {
                                    if best_difficulty.gt(&min_difficulty) {
                                        // Mine until min difficulty has been met
                                        break;
                                    }
                                } else if i == 0 {
                                    progress_bar.set_message(format!(
                                        "Mining... ({} sec remaining)",
                                        cutoff_time.saturating_sub(timer.elapsed().as_secs()),
                                    ));
                                }
                            }

                            // Increment nonce
                            nonce += 1;
                        }

                        // Return the best nonce
                        (best_nonce, best_difficulty, best_hash)
                    }
                })
            })
            .collect();

        // Join handles and return best nonce
        let mut best_nonce = 0;
        let mut best_difficulty = 0;
        let mut best_hash = Hash::default();
        for h in handles {
            if let Ok((nonce, difficulty, hash)) = h.join() {
                if difficulty > best_difficulty {
                    best_difficulty = difficulty;
                    best_nonce = nonce;
                    best_hash = hash;
                }
            }
        }

        // Update log
        progress_bar.finish_with_message(format!(
            "Best hash: {} (difficulty: {})",
            bs58::encode(best_hash.h).into_string(),
            best_difficulty
        ));

        Solution::new(best_hash.d, best_nonce.to_le_bytes())
    }

    pub fn check_num_cores(&self, threads: u64) {
        // Check num threads
        let num_cores = num_cpus::get() as u64;
        if threads.gt(&num_cores) {
            println!(
                "{} Number of threads ({}) exceeds available cores ({})",
                "WARNING".bold().yellow(),
                threads,
                num_cores
            );
        }
    }

    async fn should_reset(&self, config: Config) -> bool {
        let clock = get_clock(&self.rpc_client).await;
        config
            .last_reset_at
            .saturating_add(EPOCH_DURATION)
            .saturating_sub(5) // Buffer
            .le(&clock.unix_timestamp)
    }

    async fn get_cutoff(&self, proof: Proof, buffer_time: u64) -> u64 {
        let clock = get_clock(&self.rpc_client).await;
        proof
            .last_hash_at
            .saturating_add(60)
            .saturating_sub(buffer_time as i64)
            .saturating_sub(clock.unix_timestamp)
            .max(0) as u64
    }
}

// TODO Pick a better strategy (avoid draining bus)
fn find_bus() -> Pubkey {
    let i = rand::thread_rng().gen_range(0..BUS_COUNT);
    BUS_ADDRESSES[i]
}
