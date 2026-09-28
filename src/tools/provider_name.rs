#[derive(Clone, Copy)]
pub(crate) enum ToolNamespace {
    Skill,
    Knowledge,
    Mcp,
}

pub(crate) fn dynamic_tool_name(namespace: ToolNamespace, owner: &str, tool: &str) -> String {
    let prefix = match namespace {
        ToolNamespace::Skill => "skill",
        ToolNamespace::Knowledge => "knowledge",
        ToolNamespace::Mcp => "mcp",
    };
    let mut name = String::with_capacity(prefix.len() + 4 + owner.len() + tool.len());
    name.push_str(prefix);
    name.push_str("__");
    push_encoded(&mut name, owner);
    name.push_str("__");
    push_encoded(&mut name, tool);
    name
}

fn push_encoded(name: &mut String, component: &str) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in component.bytes() {
        if byte.is_ascii_alphanumeric() {
            name.push(char::from(byte));
        } else {
            name.push('_');
            name.push(char::from(HEX[usize::from(byte >> 4)]));
            name.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn names_are_distinct_across_providers_and_component_boundaries() {
        let inputs = ["a_b", "a.b", "a-b", "a__b", "a_2eb", "a", "b", "é", ""];
        let mut names = HashSet::new();
        for namespace in [
            ToolNamespace::Skill,
            ToolNamespace::Knowledge,
            ToolNamespace::Mcp,
        ] {
            for owner in inputs {
                for tool in inputs {
                    let name = dynamic_tool_name(namespace, owner, tool);
                    assert!(name
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'));
                    assert!(names.insert(name));
                }
            }
        }
        assert_eq!(
            dynamic_tool_name(ToolNamespace::Skill, "a.b", "c_d"),
            "skill__a_2eb__c_5fd"
        );
        assert_eq!(
            dynamic_tool_name(ToolNamespace::Mcp, "a__b", "é"),
            "mcp__a_5f_5fb___c3_a9"
        );
    }
}
