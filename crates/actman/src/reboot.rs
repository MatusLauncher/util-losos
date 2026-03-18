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
        match value.as_str() {
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
