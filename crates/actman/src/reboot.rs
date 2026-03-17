use rustix::system::RebootCommand;
use strum::{EnumIter, EnumString};
#[derive(Debug, EnumIter, PartialEq, Eq, PartialOrd, Ord, EnumString)]
pub enum RebootCMD {
    Init,
    PowerOff,
    Reboot,
    CadOff,
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
