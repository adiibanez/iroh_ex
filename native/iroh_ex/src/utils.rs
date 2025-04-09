/// Convert a string to a fixed-size 32-byte array, padding with zeros if necessary
pub fn string_to_32_byte_array(s: &str) -> [u8; 32] {
    let mut result = [0u8; 32];
    let bytes = s.as_bytes();
    let len = std::cmp::min(bytes.len(), 32);
    result[..len].copy_from_slice(&bytes[..len]);
    result
}
