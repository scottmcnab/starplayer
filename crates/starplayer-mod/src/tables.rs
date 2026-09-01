//! ProTracker's format-native lookup tables.

/// MOD signed-nibble finetune to reference rate, in file nibble order `0..+7,-8..-1`.
///
/// This is deliberately not S3M's monotonic `S2x` table.
pub const FINETUNE_REFERENCE_RATES: [u32; 16] = [
    8363, 8413, 8463, 8529, 8581, 8651, 8723, 8757,
    7895, 7941, 7985, 8046, 8107, 8169, 8232, 8280,
];

/// The original DOS loader's channel-setting map. Values below eight are left and values
/// at or above eight are right, yielding the Amiga `L-R-R-L` interleave while preserving
/// the exact 16-channel mapping used by the historical implementation.
pub const AMIGA_CHANNEL_MAP: [u8; 16] = [0, 8, 9, 1, 2, 10, 11, 3, 4, 12, 13, 5, 6, 14, 15, 7];

/// ProTracker's 16 finetuned, three-octave period tables.
///
/// Rows use the MOD header nibble order `0..+7,-8..-1`; columns are C-1 through B-3.
/// Pitch lookup, arpeggio, tone-portamento targets and glissando all index this table.
pub const PROTRACKER_PERIODS: [[u16; 36]; 16] = [
    [856,808,762,720,678,640,604,570,538,508,480,453,428,404,381,360,339,320,302,285,269,254,240,226,214,202,190,180,170,160,151,143,135,127,120,113],
    [850,802,757,715,674,637,601,567,535,505,477,450,425,401,379,357,337,318,300,284,268,253,239,225,213,201,189,179,169,159,150,142,134,126,119,113],
    [844,796,752,709,670,632,597,563,532,502,474,447,422,398,376,355,335,316,298,282,266,251,237,224,211,199,188,177,167,158,149,141,133,125,118,112],
    [838,791,746,704,665,628,592,559,528,498,470,444,419,395,373,352,332,314,296,280,264,249,235,222,209,198,187,176,166,157,148,140,132,125,118,111],
    [832,785,741,699,660,623,588,555,524,495,467,441,416,392,370,350,330,312,294,278,262,247,233,220,208,196,185,175,165,156,147,139,131,124,117,110],
    [826,779,736,694,655,619,584,551,520,491,463,437,413,390,368,347,328,309,292,276,260,245,232,219,206,195,184,174,164,155,146,138,130,123,116,109],
    [820,774,730,689,651,614,580,547,516,487,460,434,410,387,365,345,325,307,290,274,258,244,230,217,205,193,183,172,163,154,145,137,129,122,115,109],
    [814,768,725,684,646,610,575,543,513,484,457,431,407,384,363,342,323,305,288,272,256,242,228,216,204,192,181,171,161,152,144,136,128,121,114,108],
    [907,856,808,762,720,678,640,604,570,538,508,480,453,428,404,381,360,339,320,302,285,269,254,240,226,214,202,190,180,170,160,151,143,135,127,120],
    [900,850,802,757,715,675,636,601,567,535,505,477,450,425,401,379,357,337,318,300,284,268,253,238,225,212,200,189,179,169,159,150,142,134,126,119],
    [894,844,796,752,709,670,632,597,563,532,502,474,447,422,398,376,355,335,316,298,282,266,251,237,223,211,199,188,177,167,158,149,141,133,125,118],
    [887,838,791,746,704,665,628,592,559,528,498,470,444,419,395,373,352,332,314,296,280,264,249,235,222,209,198,187,176,166,157,148,140,132,125,118],
    [881,832,785,741,699,660,623,588,555,524,494,467,441,416,392,370,350,330,312,294,278,262,247,233,220,208,196,185,175,165,156,147,139,131,123,117],
    [875,826,779,736,694,655,619,584,551,520,491,463,437,413,390,368,347,328,309,292,276,260,245,232,219,206,195,184,174,164,155,146,138,130,123,116],
    [868,820,774,730,689,651,614,580,547,516,487,460,434,410,387,365,345,325,307,290,274,258,244,230,217,205,193,183,172,163,154,145,137,129,122,115],
    [862,814,768,725,684,646,610,575,543,513,484,457,431,407,384,363,342,323,305,288,272,256,242,228,216,203,192,181,171,161,152,144,136,128,121,114],
];

/// Seven-octave extension used only for the extended-range MOD path. The central three
/// octaves are exact table entries; surrounding octaves are their hardware period-domain
/// octave shifts, matching the historical `PeriodVals` scan.
pub(crate) const fn extended_period(finetune: u8, note: u8) -> u32 {
    let table = &PROTRACKER_PERIODS[(finetune & 15) as usize];
    match note {
        0..=11 => table[0] as u32 * 4,
        12..=23 => table[(note - 12) as usize] as u32 * 4,
        24..=35 => table[(note - 24) as usize] as u32 * 2,
        36..=71 => table[(note - 36) as usize] as u32,
        72..=83 => table[(note - 48) as usize] as u32 / 2,
        _ => table[((if note > 95 { 95 } else { note }) - 60) as usize] as u32 / 4,
    }
}

pub(crate) fn note_from_period(period: u16) -> Option<u8> {
    if period == 0 { return None; }
    (12..96u8).find(|note| extended_period(0, *note) <= period as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_nibble_order_is_not_the_s3m_order() {
        assert_eq!(FINETUNE_REFERENCE_RATES[15], 8280);
        assert_eq!(FINETUNE_REFERENCE_RATES[1], 8413);
    }

    #[test]
    fn standard_and_extended_periods_share_one_note_axis() {
        assert_eq!(extended_period(0, 36), 856);
        assert_eq!(extended_period(0, 71), 113);
        assert_eq!(extended_period(0, 24), 1712);
        assert_eq!(extended_period(0, 72), 107);
        assert_eq!(note_from_period(856), Some(36));
        assert_eq!(note_from_period(428), Some(48));
        assert_eq!(note_from_period(113), Some(71));
    }
}
