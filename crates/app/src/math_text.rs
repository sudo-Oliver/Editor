//! Turns the LaTeX of a formula into plain Unicode text: `\int_0^1 x^2` becomes
//! `∫₀¹ x²`, `\frac{a}{b}` becomes `a/b` and `\mathbb{R}` becomes `ℝ`. It is no
//! typesetting, but it reads as math without a layout engine.

/// The Unicode text of the formula `source`, without its dollars.
pub fn math_text(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let mut index = 0;
    let text = group(&chars, &mut index, false);
    text.split('\n').map(|line| line.split_whitespace().collect::<Vec<_>>().join(" ")).collect::<Vec<_>>().join("\n")
}

/// Reads until the end, or until the `}` that closes the group when `braced`.
fn group(chars: &[char], index: &mut usize, braced: bool) -> String {
    let mut out = String::new();
    while *index < chars.len() {
        let ch = chars[*index];
        *index += 1;
        match ch {
            '}' if braced => return out,
            '{' => out.push_str(&group(chars, index, true)),
            '^' | '_' => {
                let argument = argument(chars, index);
                out.push_str(&script(&argument, ch == '^'));
            }
            '\\' => out.push_str(&command(chars, index)),
            '~' => out.push(' '),
            '&' => out.push(' '),
            _ => out.push(ch),
        }
    }
    out
}

/// One argument: a group in braces, a command, or one char.
fn argument(chars: &[char], index: &mut usize) -> String {
    while chars.get(*index) == Some(&' ') {
        *index += 1;
    }
    match chars.get(*index) {
        Some('{') => {
            *index += 1;
            group(chars, index, true)
        }
        Some('\\') => {
            *index += 1;
            command(chars, index)
        }
        Some(&ch) => {
            *index += 1;
            ch.to_string()
        }
        None => String::new(),
    }
}

/// The command after a backslash, with its arguments.
fn command(chars: &[char], index: &mut usize) -> String {
    let start = *index;
    while chars.get(*index).is_some_and(char::is_ascii_alphabetic) {
        *index += 1;
    }
    if *index == start {
        // A one-char command: spacing, a line break or an escaped char.
        let Some(&ch) = chars.get(*index) else {
            return String::new();
        };
        *index += 1;
        return match ch {
            ',' | ':' | ';' | ' ' => "\u{2009}".to_string(),
            '!' => String::new(),
            '\\' => "\n".to_string(),
            _ => ch.to_string(),
        };
    }
    let name: String = chars[start..*index].iter().collect();
    match name.as_str() {
        "frac" | "dfrac" | "tfrac" => {
            let (top, bottom) = (argument(chars, index), argument(chars, index));
            format!("{}/{}", wrapped(&top), wrapped(&bottom))
        }
        "sqrt" => format!("√{}", wrapped(&argument(chars, index))),
        "mathbb" => argument(chars, index).chars().map(double_struck).collect(),
        "text" | "mathrm" | "mathbf" | "mathit" | "mathsf" | "mathtt" | "mathcal" | "operatorname" | "textrm" | "textbf" | "textit" | "boldsymbol" => argument(chars, index),
        "left" | "right" | "big" | "Big" | "bigg" | "Bigg" | "displaystyle" | "limits" | "nolimits" => String::new(),
        "quad" | "qquad" => " ".to_string(),
        "begin" | "end" => {
            argument(chars, index);
            String::new()
        }
        _ => symbol(&name).map_or(name, str::to_string),
    }
}

/// Parentheses around what has more than one char, as `(a+b)/2`.
fn wrapped(text: &str) -> String {
    if text.chars().count() > 1 && !text.chars().all(|ch| ch.is_alphanumeric()) { format!("({text})") } else { text.to_string() }
}

/// Superscript or subscript chars when Unicode has them all, else `^(…)`.
fn script(text: &str, superscript: bool) -> String {
    let mapped: Option<String> = text.chars().map(|ch| if superscript { superscript_char(ch) } else { subscript_char(ch) }).collect();
    mapped.unwrap_or_else(|| {
        let mark = if superscript { '^' } else { '_' };
        if text.chars().count() == 1 { format!("{mark}{text}") } else { format!("{mark}({text})") }
    })
}

fn superscript_char(ch: char) -> Option<char> {
    Some(match ch {
        '0' => '⁰', '1' => '¹', '2' => '²', '3' => '³', '4' => '⁴', '5' => '⁵', '6' => '⁶', '7' => '⁷', '8' => '⁸', '9' => '⁹',
        '+' => '⁺', '-' | '−' => '⁻', '=' => '⁼', '(' => '⁽', ')' => '⁾', 'n' => 'ⁿ', 'i' => 'ⁱ', 'T' => 'ᵀ', '*' | '∗' => '*',
        '′' => '′', 'a' => 'ᵃ', 'b' => 'ᵇ', 'c' => 'ᶜ', 'd' => 'ᵈ', 'e' => 'ᵉ', 'k' => 'ᵏ', 'm' => 'ᵐ', 'x' => 'ˣ', 'y' => 'ʸ',
        _ => return None,
    })
}

fn subscript_char(ch: char) -> Option<char> {
    Some(match ch {
        '0' => '₀', '1' => '₁', '2' => '₂', '3' => '₃', '4' => '₄', '5' => '₅', '6' => '₆', '7' => '₇', '8' => '₈', '9' => '₉',
        '+' => '₊', '-' | '−' => '₋', '=' => '₌', '(' => '₍', ')' => '₎',
        'a' => 'ₐ', 'e' => 'ₑ', 'h' => 'ₕ', 'i' => 'ᵢ', 'j' => 'ⱼ', 'k' => 'ₖ', 'l' => 'ₗ', 'm' => 'ₘ', 'n' => 'ₙ', 'o' => 'ₒ',
        'p' => 'ₚ', 'r' => 'ᵣ', 's' => 'ₛ', 't' => 'ₜ', 'u' => 'ᵤ', 'v' => 'ᵥ', 'x' => 'ₓ',
        _ => return None,
    })
}

fn double_struck(ch: char) -> char {
    match ch {
        'N' => 'ℕ', 'Z' => 'ℤ', 'Q' => 'ℚ', 'R' => 'ℝ', 'C' => 'ℂ', 'P' => 'ℙ', 'H' => 'ℍ', 'E' => '𝔼',
        _ => ch,
    }
}

fn symbol(name: &str) -> Option<&'static str> {
    Some(match name {
        "alpha" => "α", "beta" => "β", "gamma" => "γ", "delta" => "δ", "epsilon" => "ϵ", "varepsilon" => "ε", "zeta" => "ζ",
        "eta" => "η", "theta" => "θ", "vartheta" => "ϑ", "iota" => "ι", "kappa" => "κ", "lambda" => "λ", "mu" => "μ", "nu" => "ν",
        "xi" => "ξ", "pi" => "π", "varpi" => "ϖ", "rho" => "ρ", "varrho" => "ϱ", "sigma" => "σ", "varsigma" => "ς", "tau" => "τ",
        "upsilon" => "υ", "phi" => "ϕ", "varphi" => "φ", "chi" => "χ", "psi" => "ψ", "omega" => "ω",
        "Gamma" => "Γ", "Delta" => "Δ", "Theta" => "Θ", "Lambda" => "Λ", "Xi" => "Ξ", "Pi" => "Π", "Sigma" => "Σ",
        "Upsilon" => "Υ", "Phi" => "Φ", "Psi" => "Ψ", "Omega" => "Ω",
        "int" => "∫", "iint" => "∬", "iiint" => "∭", "oint" => "∮", "sum" => "∑", "prod" => "∏", "coprod" => "∐",
        "infty" => "∞", "partial" => "∂", "nabla" => "∇", "cdot" => "·", "times" => "×", "div" => "÷", "pm" => "±", "mp" => "∓",
        "leq" | "le" => "≤", "geq" | "ge" => "≥", "neq" | "ne" => "≠", "approx" => "≈", "equiv" => "≡", "sim" => "∼",
        "simeq" => "≃", "cong" => "≅", "propto" => "∝", "ll" => "≪", "gg" => "≫",
        "in" => "∈", "notin" => "∉", "ni" => "∋", "subset" => "⊂", "supset" => "⊃", "subseteq" => "⊆", "supseteq" => "⊇",
        "cup" => "∪", "cap" => "∩", "setminus" => "∖", "emptyset" | "varnothing" => "∅",
        "forall" => "∀", "exists" => "∃", "nexists" => "∄", "neg" | "lnot" => "¬", "land" | "wedge" => "∧", "lor" | "vee" => "∨",
        "to" | "rightarrow" => "→", "leftarrow" | "gets" => "←", "leftrightarrow" => "↔", "Rightarrow" | "implies" => "⇒",
        "Leftarrow" => "⇐", "Leftrightarrow" | "iff" => "⇔", "mapsto" => "↦", "uparrow" => "↑", "downarrow" => "↓",
        "ldots" | "dots" => "…", "cdots" => "⋯", "vdots" => "⋮", "ddots" => "⋱",
        "circ" => "∘", "bullet" => "•", "star" => "⋆", "ast" => "∗", "oplus" => "⊕", "otimes" => "⊗", "perp" => "⊥",
        "parallel" => "∥", "mid" => "∣", "angle" => "∠", "triangle" => "△", "hbar" => "ħ", "ell" => "ℓ", "Re" => "ℜ",
        "Im" => "ℑ", "aleph" => "ℵ", "prime" => "′", "degree" => "°",
        "langle" => "⟨", "rangle" => "⟩", "lfloor" => "⌊", "rfloor" => "⌋", "lceil" => "⌈", "rceil" => "⌉",
        "lbrace" => "{", "rbrace" => "}", "vert" | "lvert" | "rvert" => "|", "Vert" | "lVert" | "rVert" => "‖",
        "sin" => "sin", "cos" => "cos", "tan" => "tan", "log" => "log", "ln" => "ln", "exp" => "exp", "lim" => "lim",
        "max" => "max", "min" => "min", "sup" => "sup", "inf" => "inf", "det" => "det",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formulas_become_unicode() {
        assert_eq!(math_text(r"\int_0^1 x^2 \, dx = \frac{1}{3}"), "∫₀¹ x² dx = 1/3");
        assert_eq!(math_text(r"E = mc^2"), "E = mc²");
        assert_eq!(math_text(r"\alpha_{i+1} \leq \sqrt{a+b}"), "αᵢ₊₁ ≤ √(a+b)");
        assert_eq!(math_text(r"\frac{a+b}{2} \in \mathbb{R}"), "(a+b)/2 ∈ ℝ");
        assert_eq!(math_text(r"x^{\pi}"), "x^π");
        assert_eq!(math_text(r"\sum_{k=1}^{n} k"), "∑ₖ₌₁ⁿ k");
        assert_eq!(math_text(r"a \\ b"), "a\nb");
    }
}
