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
