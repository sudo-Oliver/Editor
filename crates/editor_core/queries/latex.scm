; Later and narrower captures win when the view flattens highlights.
(inline_formula) @markup.math
(displayed_equation) @markup.math
(math_environment) @markup.math

(command_name) @function
(line_comment) @comment
(block_comment) @comment
(comment_environment) @comment

(begin command: _ @function)
(end command: _ @function)
(begin name: (curly_group_text (text) @type))
(end name: (curly_group_text (text) @type))

(part command: _ @function text: (curly_group) @markup.heading)
(chapter command: _ @function text: (curly_group) @markup.heading)
(section command: _ @function text: (curly_group) @markup.heading)
(subsection command: _ @function text: (curly_group) @markup.heading)
(subsubsection command: _ @function text: (curly_group) @markup.heading)
(paragraph command: _ @function)
(subparagraph command: _ @function)
(title_declaration command: _ @function text: (curly_group) @markup.heading)

(class_include command: _ @keyword options: (brack_group_key_value)? @string)
(package_include command: _ @keyword options: (brack_group_key_value)? @string)
(latex_include command: _ @function path: (curly_group_path) @string)
(graphics_include command: _ @function path: (curly_group_path) @string)
(bibtex_include command: _ @function)

(citation command: _ @keyword keys: (curly_group_text_list) @string)
(label_definition command: _ @keyword name: (curly_group_label) @string)
(label_reference command: _ @keyword names: (curly_group_label_list) @string)
(hyperlink command: _ @function uri: (curly_group_uri) @link)
(new_command_definition command: _ @keyword)

; These commands are grammar tokens, so `command_name` does not match them.
(author_declaration command: _ @function)
(caption command: _ @function)
(svg_include command: _ @function)
(inkscape_include command: _ @function)
(verbatim_include command: _ @function)
(import_include command: _ @function)
(bibstyle_include command: _ @function)
(color_reference command: _ @function)
(text_mode command: _ @function)
(changes_replaced command: _ @function)
(counter_value command: _ @function)
(counter_typesetting command: _ @function)
(counter_definition command: _ @function)
(counter_addition command: _ @function)
(counter_increment command: _ @function)

(label_reference_range command: _ @keyword)
(label_number command: _ @keyword)
(old_command_definition command: _ @keyword)
(let_command_definition command: _ @keyword)
(paired_delimiter_definition command: _ @keyword)
(environment_definition command: _ @keyword)
(glossary_entry_definition command: _ @keyword)
(glossary_entry_reference command: _ @keyword)
(acronym_definition command: _ @keyword)
(acronym_reference command: _ @keyword)
(theorem_definition command: _ @keyword)
(color_definition command: _ @keyword)
(color_set_definition command: _ @keyword)
(counter_declaration command: _ @keyword)
(counter_within_declaration command: _ @keyword)
(counter_without_declaration command: _ @keyword)
(tikz_library_import command: _ @keyword)
