//! `ricow deploy <preview_id> --token <t>` — 落盘已批准的建策略 preview (002 第三步)。
//!
//! token 只能由 `ricow approve` 从 pending 状态生成, 且一次性; 无 token 无法部署 (写操作不得一步落盘)。

use clap::Args;
use ricow_core::{CoreError, CoreResult};
use ricow_strategy::Database;

use crate::commands::{default_db_path, ensure_strategies_dir};

#[derive(Args)]
pub struct DeployArgs {
    /// preview id (由 `ricow create` 返回)
    pub preview_id: String,
    /// 一次性 token (由 `ricow approve` 返回)
    #[arg(long)]
    pub token: String,
}

pub async fn run(args: DeployArgs) -> CoreResult<()> {
    let db =
        Database::open(&default_db_path()).await.map_err(|e| CoreError::Exchange(e.to_string()))?;
    let dir = ensure_strategies_dir()?;

    let (toml_path, lua_path) =
        ricow_engine::execute_strategy(&db, &args.preview_id, &args.token, &dir).await?;

    let name = toml_path.file_stem().and_then(|s| s.to_str()).unwrap_or("<name>").to_string();
    println!("已部署:");
    println!("  {}", toml_path.display());
    println!("  {}", lua_path.display());
    println!("下一步:");
    println!("  1) Dry Run 观察(真实行情, 虚拟成交): ricow run {name}");
    println!(
        "  2) 切实盘: TOML `live_enabled = true` + `ricow run {name} --live`; \
         首次前需 Dry Run 满 `params.min_dry_run_hours`(默认 24 小时, 可设 0 关闭)"
    );
    Ok(())
}
