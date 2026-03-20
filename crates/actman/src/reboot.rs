use rustix::system::RebootCommand;
use strum::EnumIter;
#[derive(Debug, EnumIter, PartialEq, Eq, PartialOrd, Ord)]
pub enum RebootCMD {
    Init,
    PowerOff,
    Reboot,
    CadOff,
}

impl<'a> From<&'a String> for RebootCMD {
    fn from(value: &'a String) -> Self {
        let basename = std::path::Path::new(value)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(value.as_str());
        match basename {
            "init" => Self::Init,
            "poweroff" => Self::PowerOff,
            "reboot" => Self::Reboot,
            _ => Self::CadOff,
        }
    }
}

impl From<RebootCommand> for RebootCMD {
    fn from(value: RebootCommand) -> Self {
        match value {
            RebootCommand::Restart => Self::Reboot,
            RebootCommand::PowerOff => Self::PowerOff,
            _ => Self::CadOff,
        }
    }
}
impl From<RebootCMD> for RebootCommand {
    fn from(value: RebootCMD) -> Self {
        match value {
            RebootCMD::Reboot => Self::Restart,
            RebootCMD::PowerOff => Self::PowerOff,
            _ => Self::CadOff,
        }
    }
}
