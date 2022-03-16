#![feature(let_chains)]

use std::collections::HashMap;
use std::env;
use std::fmt::Write;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::Result;
use console::style;
use indicatif::{MultiProgress, ProgressBar};
use risedev::util::complete_spin;
use risedev::{
    AwsS3Config, ComputeNodeService, ConfigExpander, ConfigureTmuxTask, EnsureStopService,
    ExecuteContext, FrontendService, FrontendServiceV2, GrafanaService, JaegerService,
    MetaNodeService, MinioService, PrometheusService, ServiceConfig, Task, RISEDEV_SESSION_NAME,
};
use tempfile::tempdir;
use yaml_rust::YamlEmitter;

#[derive(Default)]
pub struct ProgressManager {
    mp: Arc<MultiProgress>,
    pa: Vec<ProgressBar>,
    insert: Option<usize>,
}

impl ProgressManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new progress bar from task
    pub fn new_progress(&mut self) -> ProgressBar {
        let pb = risedev::util::new_spinner();
        if let Some(ref mut insert) = self.insert {
            self.mp.insert(*insert, pb.clone());
            *insert += 1;
        } else {
            self.mp.add(pb.clone());
            self.insert = Some(0);
        }
        self.pa.push(pb.clone());
        pb.enable_steady_tick(100);
        pb
    }

    /// Finish all progress bars.
    pub fn finish_all(&self) {
        for p in &self.pa {
            p.finish();
        }
    }

    pub fn spawn(&self) -> JoinHandle<anyhow::Result<()>> {
        let mp = self.mp.clone();
        std::thread::spawn(move || mp.join().map_err(|err| err.into()))
    }
}

fn task_main(
    manager: &mut ProgressManager,
    steps: &[String],
    services: &HashMap<String, ServiceConfig>,
) -> Result<(Vec<(String, Duration)>, String)> {
    let log_path = env::var("PREFIX_LOG")?;

    let mut logger = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(Path::new(&log_path).join("risedev.log"))?;

    let status_dir = Arc::new(tempdir()?);

    let mut log_buffer = String::new();

    // Start Tmux and kill previous services
    {
        let mut ctx = ExecuteContext::new(&mut logger, manager.new_progress(), status_dir.clone());
        let mut service = ConfigureTmuxTask::new()?;
        service.execute(&mut ctx)?;

        writeln!(
            log_buffer,
            "* Run {} to attach to the tmux console.",
            style(format!("tmux a -t {}", RISEDEV_SESSION_NAME))
                .blue()
                .bold()
        )?;
    }

    // Firstly, ensure that all ports needed is not occupied by previous runs.
    let mut ports = vec![];

    for step in steps {
        let service = services.get(step).unwrap();
        let listen_info = match service {
            ServiceConfig::Minio(c) => Some((c.port, c.id.clone())),
            ServiceConfig::Etcd(c) => Some((c.port, c.id.clone())),
            ServiceConfig::Prometheus(c) => Some((c.port, c.id.clone())),
            ServiceConfig::ComputeNode(c) => Some((c.port, c.id.clone())),
            ServiceConfig::MetaNode(c) => Some((c.port, c.id.clone())),
            ServiceConfig::Frontend(c) => Some((c.port, c.id.clone())),
            ServiceConfig::FrontendV2(c) => Some((c.port, c.id.clone())),
            ServiceConfig::Grafana(c) => Some((c.port, c.id.clone())),
            ServiceConfig::Jaeger(c) => Some((c.dashboard_port, c.id.clone())),
            ServiceConfig::AwsS3(_) => None,
        };

        if let Some(x) = listen_info {
            ports.push(x);
        }
    }

    {
        let mut ctx = ExecuteContext::new(&mut logger, manager.new_progress(), status_dir.clone());
        let mut service = EnsureStopService::new(ports)?;
        service.execute(&mut ctx)?;
    }

    // Then, start services one by one

    let mut stat = vec![];

    for step in steps {
        let service = services.get(step).unwrap();
        let start_time = Instant::now();

        match service {
            ServiceConfig::Minio(c) => {
                let mut ctx =
                    ExecuteContext::new(&mut logger, manager.new_progress(), status_dir.clone());
                let mut service = MinioService::new(c.clone())?;
                service.execute(&mut ctx)?;

                let mut task = risedev::ConfigureMinioTask::new(c.clone())?;
                task.execute(&mut ctx)?;
            }
            ServiceConfig::Etcd(c) => {
                let mut ctx =
                    ExecuteContext::new(&mut logger, manager.new_progress(), status_dir.clone());
                let mut service = risedev::EtcdService::new(c.clone())?;
                service.execute(&mut ctx)?;

                let mut task = risedev::EtcdReadyCheckTask::new(c.clone())?;
                task.execute(&mut ctx)?;
            }
            ServiceConfig::Prometheus(c) => {
                let mut ctx =
                    ExecuteContext::new(&mut logger, manager.new_progress(), status_dir.clone());
                let mut service = PrometheusService::new(c.clone())?;
                service.execute(&mut ctx)?;
                let mut task = risedev::ConfigureGrpcNodeTask::new(c.port, false)?;
                task.execute(&mut ctx)?;
                ctx.pb
                    .set_message(format!("api http://{}:{}/", c.address, c.port));
            }
            ServiceConfig::ComputeNode(c) => {
                let mut ctx =
                    ExecuteContext::new(&mut logger, manager.new_progress(), status_dir.clone());
                let mut service = ComputeNodeService::new(c.clone())?;
                service.execute(&mut ctx)?;

                let mut task = risedev::ConfigureGrpcNodeTask::new(c.port, c.user_managed)?;
                task.execute(&mut ctx)?;
                ctx.pb
                    .set_message(format!("api grpc://{}:{}/", c.address, c.port));
            }
            ServiceConfig::MetaNode(c) => {
                let mut ctx =
                    ExecuteContext::new(&mut logger, manager.new_progress(), status_dir.clone());
                let mut service = MetaNodeService::new(c.clone())?;
                service.execute(&mut ctx)?;
                let mut task = risedev::ConfigureGrpcNodeTask::new(c.port, c.user_managed)?;
                task.execute(&mut ctx)?;
                ctx.pb.set_message(format!(
                    "api grpc://{}:{}/, dashboard http://{}:{}/",
                    c.address, c.port, c.dashboard_address, c.dashboard_port
                ));
            }
            ServiceConfig::Frontend(c) => {
                let mut ctx =
                    ExecuteContext::new(&mut logger, manager.new_progress(), status_dir.clone());
                let mut service = FrontendService::new(c.clone())?;
                service.execute(&mut ctx)?;
                let mut task = risedev::ConfigureGrpcNodeTask::new(c.port, c.user_managed)?;
                task.execute(&mut ctx)?;
                ctx.pb
                    .set_message(format!("api postgres://{}:{}/", c.address, c.port));

                writeln!(
                    log_buffer,
                    "* Run {} to start Postgres interactive shell.",
                    style(format!("psql -h localhost -p {} -d dev", c.port))
                        .blue()
                        .bold()
                )?;
            }
            ServiceConfig::FrontendV2(c) => {
                let mut ctx =
                    ExecuteContext::new(&mut logger, manager.new_progress(), status_dir.clone());
                let mut service = FrontendServiceV2::new(c.clone())?;
                service.execute(&mut ctx)?;
                let mut task = risedev::ConfigureGrpcNodeTask::new(c.port, c.user_managed)?;
                task.execute(&mut ctx)?;
                ctx.pb
                    .set_message(format!("api postgres://{}:{}/", c.address, c.port));

                writeln!(
                    log_buffer,
                    "* Run {} to start Postgres interactive shell.",
                    style(format!("psql -h localhost -p {} -d dev", c.port))
                        .blue()
                        .bold()
                )?;
            }
            ServiceConfig::Grafana(c) => {
                let mut ctx =
                    ExecuteContext::new(&mut logger, manager.new_progress(), status_dir.clone());
                let mut service = GrafanaService::new(c.clone())?;
                service.execute(&mut ctx)?;
                let mut task = risedev::ConfigureGrpcNodeTask::new(c.port, false)?;
                task.execute(&mut ctx)?;
                ctx.pb
                    .set_message(format!("dashboard http://{}:{}/", c.address, c.port));
            }
            ServiceConfig::Jaeger(c) => {
                let mut ctx =
                    ExecuteContext::new(&mut logger, manager.new_progress(), status_dir.clone());
                let mut service = JaegerService::new(c.clone())?;
                service.execute(&mut ctx)?;
                let mut task = risedev::ConfigureGrpcNodeTask::new(c.dashboard_port, false)?;
                task.execute(&mut ctx)?;
                ctx.pb.set_message(format!(
                    "dashboard http://{}:{}/",
                    c.dashboard_address, c.dashboard_port
                ));
            }
            ServiceConfig::AwsS3(c) => {
                let mut ctx =
                    ExecuteContext::new(&mut logger, manager.new_progress(), status_dir.clone());

                struct AwsService(AwsS3Config);
                impl Task for AwsService {
                    fn execute(
                        &mut self,
                        _ctx: &mut ExecuteContext<impl std::io::Write>,
                    ) -> anyhow::Result<()> {
                        Ok(())
                    }

                    fn id(&self) -> String {
                        self.0.id.clone()
                    }
                }

                ctx.service(&AwsService(c.clone()));
                ctx.complete_spin();
                ctx.pb
                    .set_message(format!("using AWS s3 bucket {}", c.bucket));
            }
        }

        let service_id = service.id().to_string();
        let duration = Instant::now() - start_time;
        stat.push((service_id, duration));
    }

    Ok((stat, log_buffer))
}

fn preflight_check() {
    if env::var("http_proxy").is_ok()
        || env::var("https_proxy").is_ok()
        || env::var("HTTP_PROXY").is_ok()
        || env::var("HTTPS_PROXY").is_ok()
        || env::var("all_proxy").is_ok()
        || env::var("ALL_PROXY").is_ok()
    {
        if let Ok(x) = env::var("no_proxy") && x.contains("127.0.0.1") && x.contains("::1") {
            println!(
                "[{}] {} - You are using proxies for all RisingWave components. Please make sure that `no_proxy` is set for all worker nodes within the cluster.",
                style("risedev-preflight-check").bold(),
                style("WARN").yellow().bold()
            );
        } else {
            println!(
                "[{}] {} - `no_proxy` is not set correctly, which might cause failure in RiseDev and RisingWave. Consider {}.",
                style("risedev-preflight-check").bold(),
                style("WARN").yellow().bold(),
                style("`export no_proxy=localhost,127.0.0.1,::1`").blue().bold()
            );
        }
    }
}

fn main() -> Result<()> {
    let risedev_config = {
        let mut content = String::new();
        File::open("risedev.yml")?.read_to_string(&mut content)?;
        content
    };
    let risedev_config = ConfigExpander::expand(&risedev_config)?;
    {
        let mut out_str = String::new();
        let mut emitter = YamlEmitter::new(&mut out_str);
        emitter.dump(&risedev_config)?;
        std::fs::write(
            Path::new(&env::var("PREFIX_CONFIG")?).join("risedev-expanded.yml"),
            &out_str,
        )?;
    }

    preflight_check();

    let task_name = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "default".to_string());
    let (steps, services) = ConfigExpander::select(&risedev_config, &task_name)?;

    let mut manager = ProgressManager::new();
    // Always create a progress before calling `task_main`. Otherwise the progress bar won't be
    // shown.
    let p = manager.new_progress();
    p.set_prefix("playground");
    p.set_message(format!(
        "starting {} services for {}...",
        steps.len(),
        task_name
    ));
    let join_handle = manager.spawn();
    let task_result = task_main(&mut manager, &steps, &services);
    p.set_message(format!("done bootstrapping {}", task_name));
    complete_spin(&p);
    manager.finish_all();
    join_handle.join().unwrap()?;

    let log_path = env::var("PREFIX_LOG")?;

    match task_result {
        Ok((stat, log_buffer)) => {
            println!("--- summary of startup time ---");
            for (task_name, duration) in stat {
                println!("{}: {:.2}s", task_name, duration.as_secs_f64());
            }
            println!("-------------------------------");
            println!();

            println!("All services started successfully.");

            print!("{}", log_buffer);

            println!("* You may find logs at {}", style(log_path).blue().bold());

            println!(
                "* Run {} or {} to kill cluster.",
                style("./risedev kill").blue().bold(),
                style("./risedev k").blue().bold()
            );

            Ok(())
        }
        Err(err) => {
            println!("* Failed to start: {}", err.root_cause().to_string().trim(),);
            println!(
                "please refer to logs for more information {}",
                env::var("PREFIX_LOG")?
            );
            println!("* Run `./risedev kill` or `./risedev k` to clean up cluster.");
            println!("---");
            println!();
            println!();

            Err(err)
        }
    }
}