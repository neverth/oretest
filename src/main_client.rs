use warp::Filter;
use std::net::TcpListener;
use std::io::{Read, Write};
use std::str::FromStr;
use std::time::Instant;
use drillx::{equix, Hash, Solution};
use serde::{Deserialize, Serialize};
use crate::args::ClaimArgs;
use crate::Miner;


#[derive(Debug, Deserialize)]
struct QueryParams {
    cutoff_time: Option<u64>,
    threads: Option<u64>,
    min_difficulty: Option<u64>,
    challenge: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
struct Response {
    d: [u8; 16],
    n: [u8; 8],
    challenge: String,
    best_difficulty: u64,
}


impl Miner {
    pub async fn client(&self, args: ClaimArgs) {
        let routes = warp::get()
            .and(warp::path("ore"))
            .and(warp::query::<QueryParams>())
            .map(|params: QueryParams| {
                println!("request {:?}", params);

                let challenge: [u8; 32] = params.challenge.unwrap()
                    .trim_matches(|c| c == '[' || c == ']')
                    .split(", ")
                    .map(|x| u8::from_str(x).unwrap())
                    .collect::<Vec<_>>()
                    .try_into()
                    .unwrap();

                // println!("input challenge {:?}", challenge);

                // let challenge: [u8; 32] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31];
                // let string = String::from_utf8(challenge.to_vec()).unwrap();
                // println!("111 {}", string);
                //
                // let bytes: [u8; 32] = string.as_bytes().try_into().unwrap();
                // println!("222 {:?}", bytes);

                let (solution, best_difficulty) = find_hash_par(
                    challenge,
                    params.cutoff_time.unwrap_or(0),
                    params.threads.unwrap_or(0),
                    params.min_difficulty.unwrap_or(0) as u32,
                );

                let response = Response {
                    d: solution.d,
                    n: solution.n,
                    challenge: format!("{:?}", challenge),
                    best_difficulty,
                };

                println!("resp {:?}", response);

                warp::reply::json(&response)
            });

        warp::serve(routes).run(([0, 0, 0, 0], 6789)).await;
    }
}


fn find_hash_par(
    challenge: [u8; 32],
    cutoff_time: u64,
    threads: u64,
    min_difficulty: u32,
) -> (Solution, u64) {
    // Dispatch job to each thread
    let handles: Vec<_> = (0..threads)
        .map(|i| {
            std::thread::spawn({
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
                            &challenge,
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
                                println!("Mining... ({} sec remaining), difficulty, {}", cutoff_time.saturating_sub(timer.elapsed().as_secs()), best_difficulty)
                            }
                        }

                        if nonce % 100 == 0 && i == 0 {}

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
    // println!(format!(
    //     "Best hash: {} (difficulty: {})",
    //     bs58::encode(best_hash.h).into_string(),
    //     best_difficulty
    // ));

    (Solution::new(best_hash.d, best_nonce.to_le_bytes()), best_difficulty as u64)
}