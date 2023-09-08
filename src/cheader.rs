use std::collections::HashMap;
use std::io::BufReader;
use std::fs;
use serde_json::Value;
use eyre::bail;
use alloy_json_abi::{JsonAbi, Function};

pub fn c_headers(in_path: String, out_path: String) ->eyre::Result<()> {
    let f = fs::File::open(&in_path)?;

    let input: Value = serde_json::from_reader(BufReader::new(f))?;
    
    let Some(input_contracts) = input["contracts"].as_object() else {
        bail!("did not find top-level contracts object in {}", in_path)
    };

    let mut pathbuf = std::path::PathBuf::new();
    pathbuf.push(out_path);
    for (solidity_file_name, solidity_file_out) in input_contracts.iter() {
        let debug_path = vec![solidity_file_name.as_str()];
        let Some(contracts) = solidity_file_out.as_object() else {
            println!("skipping output for {:?} not an object..", &debug_path);
            continue;
        };
        pathbuf.push(&solidity_file_name);
        fs::create_dir_all(&pathbuf)?;
        for (contract_name, contract_val) in contracts.iter() {
            let mut debug_path = debug_path.clone();
            debug_path.push(&contract_name);
            let Some(properties) = contract_val.as_object() else {
                println!("skipping output for {:?} not an object..", &debug_path);
                continue;
            };
            
            let mut methods :HashMap<String, Vec<Function>> = HashMap::default();

            if let Some(raw) = properties.get("abi") {
                // Sadly, JsonAbi = serde_json::from_value is not supported.
                // Tonight, we hack!
                let abi_json = serde_json::to_string(raw)?;
                let abi:JsonAbi = serde_json::from_str(&abi_json)?;
                for function in abi.functions() {
                    let name = function.name.clone();
                    methods.entry(name).or_insert(Vec::default()).push(function.clone());
                }    
            } else {
                println!("skipping abi for {:?}: not found", &debug_path);               
            }

            let mut header_body = String::default();
            let mut router_body = String::default();

            for (simple_name, mut overloads) in methods {
                overloads.sort_by(|a, b| a.signature().cmp(&b.signature()));
                for (index, overload) in overloads.iter().enumerate() {
                    let c_name = match index {
                        0 => simple_name.clone(),
                        x => format!("{}_{}",simple_name, x),
                    };
                    let selector = u32::from_be_bytes(overload.selector());
                    header_body.push_str(format!("#define SELECTOR_{} 0x{:08x} // {}\n", c_name, selector, overload.signature()).as_str());
                    header_body.push_str(format!("ArbResult {}(uint8_t *input, size_t len); // {}\n", c_name, overload.signature()).as_str());
                    router_body.push_str(format!("    if (selector==SELECTOR_{}) return {}(input, len);\n", c_name, c_name).as_str());
                }
            }

            if header_body.len() != 0 {
                header_body.push('\n');
            }
            debug_path.push("storageLayout");
            if let Some(Value::Object(layout_vals)) = properties.get("storageLayout") {
                debug_path.push("storage");
                if let Some(Value::Array(storage_arr)) = layout_vals.get("storage") {
                    for storage_val in storage_arr.iter() {
                        let Some(storage_obj) = storage_val.as_object() else {
                            println!("skipping output inside {:?}: not an object..", &debug_path);
                            continue;
                        };
                        let Some(Value::String(label)) = storage_obj.get("label") else {
                            println!("skipping output inside {:?}: no label..", &debug_path);
                            continue;
                        };
                        let Some(Value::String(slot)) = storage_obj.get("slot") else {
                            println!("skipping output inside {:?}: no slot..", &debug_path);
                            continue;
                        };
                        let Some(Value::Number(offset)) = storage_obj.get("offset") else {
                            println!("skipping output inside {:?}: no offset..", &debug_path);
                            continue;
                        };
                        header_body.push_str("#define STORAGE_SLOT_");
                        header_body.push_str(&label);
                        header_body.push(' ');
                        header_body.push_str(&slot);
                        header_body.push('\n');
                        header_body.push_str("#define STORAGE_OFFSET_");
                        header_body.push_str(&label);
                        header_body.push(' ');
                        header_body.push_str(offset.to_string().as_str());
                        header_body.push('\n');
                    }
                } else {
                    println!("skipping output for {:?}: not an array..", &debug_path);
                }
                debug_path.pop();
            } else {
                println!("skipping output for {:?}: not an object..", &debug_path);
            }
            debug_path.pop();
            if header_body.len() != 0 {
                let mut unique_identifier = String::from("__");
                unique_identifier.push_str(&solidity_file_name.to_uppercase());
                unique_identifier.push('_');
                unique_identifier.push_str(&contract_name.to_uppercase());
                unique_identifier.push('_');

                let contents = format!(r#" // autogenerated by cargo-stylus
#ifndef {uniq}
#define {uniq}

#include <stylus.h>

#ifdef __cplusplus
extern "C" {{
#endif

{body}

#ifdef __cplusplus
}}
#endif

#endif // {uniq}
"#
                ,uniq=unique_identifier, body=header_body);

                let filename :String = contract_name.into();
                pathbuf.push(filename + ".h");
                fs::write(&pathbuf, &contents)?;
                pathbuf.pop();   
            }
            if router_body.len() != 0 {
                let contents = format!(r#" // autogenerated by cargo-stylus

#include "{contract}.h"
#include <stylus.h>
#include <bebi.h>

ArbResult {contract}_entry(uint8_t *input, size_t len) {{
    ArbResult err = {{Failure, 0, 0}};
    if (len < 4) {{
        return err;
    }}
    uint32_t selector = bebi_get_u32(input, 0);
    input +=4;
    len -=4;
{body}
    return err;
}}

ENTRYPOINT({contract}_entry)
"#
            ,contract=contract_name, body=router_body);

            let filename :String = contract_name.into();
            pathbuf.push(filename + "_main.c");
            fs::write(&pathbuf, &contents)?;
            pathbuf.pop();   

        }
        }
        pathbuf.pop();
    }
    Ok(())
}