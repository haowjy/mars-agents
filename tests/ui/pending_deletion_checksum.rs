use mars_agents::lock::OutputRecord;

fn main() {
    let pending = OutputRecord::pending_deletion(".opencode", "plugins/mars-audit.ts");

    let _checksum = pending.installed_checksum;
}
