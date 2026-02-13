use acvm::acir::{
    native_types::{Witness, WitnessMap, WitnessStack},
    FieldElement,
};
use flate2::bufread::{GzDecoder, GzEncoder};
use flate2::Compression;
use std::io::Read;

/// Convert a vector of field elements to a witness map
///
/// # Arguments
///
/// * witness_vec: The vector of field elements to convert to a witness map
/// The actual type of the vector items can be either a FieldElement or an unsigned integer
///
/// # Returns
///
/// The witness map
pub fn from_vec_to_witness_map<T>(witness_vec: Vec<T>) -> Result<WitnessMap<FieldElement>, String>
where
    T: Copy,
    FieldElement: From<T>,
{
    let mut witness_map = WitnessMap::new();

    for (i, witness) in witness_vec.iter().enumerate() {
        witness_map.insert(Witness(i as u32), FieldElement::from(*witness));
    }

    Ok(witness_map)
}

/// Convert a vector of strings to a witness map
///
/// # Arguments
///
/// * witness_vec: The vector of strings to convert to a witness map
/// Each string is expected to be a valid hexadecimal or decimal string
///
/// # Returns
///
/// The witness map
pub fn from_vec_str_to_witness_map(
    witness_vec: Vec<&str>,
) -> Result<WitnessMap<FieldElement>, String> {
    let mut witness_map = WitnessMap::new();

    for (i, witness) in witness_vec.iter().enumerate() {
        witness_map.insert(
            Witness(i as u32),
            FieldElement::try_from_str(*witness).unwrap_or_default(),
        );
    }

    Ok(witness_map)
}

/// Wrap the witness map into a witness stack
///
/// # Arguments
///
/// * witness_map: The witness map to wrap into a witness stack
///
/// # Returns
///
/// The witness stack
pub fn witness_map_to_witness_stack(
    witness_map: WitnessMap<FieldElement>,
) -> Result<WitnessStack<FieldElement>, String> {
    let witness_stack = WitnessStack::try_from(witness_map).map_err(|e| e.to_string())?;
    Ok(witness_stack)
}

/// Serialize the witness stack into the raw (gunzipped) bytes expected by bb.
///
/// # Arguments
///
/// * witness_stack: The witness stack to serialize
///
/// # Returns
///
/// The serialized witness stack
pub fn serialize_witness(witness_stack: WitnessStack<FieldElement>) -> Result<Vec<u8>, String> {
    let compressed = witness_stack.serialize().map_err(|e| e.to_string())?;

    let mut decoder = GzDecoder::new(compressed.as_slice());
    let mut raw = Vec::new();
    decoder.read_to_end(&mut raw).map_err(|e| e.to_string())?;

    Ok(raw)
}

/// Deserialize a witness stack from either compressed or raw serialized bytes.
///
/// # Arguments
///
/// * serialized_witness_stack: The serialized witness stack to deserialize
///
/// # Returns
///
/// The witness stack
pub fn deserialize_witness(
    serialized_witness_stack: Vec<u8>,
) -> Result<WitnessStack<FieldElement>, String> {
    if let Ok(stack) = WitnessStack::deserialize(&serialized_witness_stack) {
        return Ok(stack);
    }

    let mut encoder = GzEncoder::new(serialized_witness_stack.as_slice(), Compression::best());
    let mut compressed = Vec::new();
    encoder
        .read_to_end(&mut compressed)
        .map_err(|e| e.to_string())?;

    WitnessStack::deserialize(&compressed).map_err(|e| e.to_string())
}
