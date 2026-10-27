; LaTeX, coloured as a reader of the source looks for it: commands as
; functions, the words that structure a document — \section, \begin,
; \label, \cite, \usepackage — as keywords, headings as headings, the names
; of environments and packages as types, labels and keys as labels, and
; mathematics and comments as what they are.

[(line_comment) (block_comment) (comment_environment)] @comment
(verbatim_environment verbatim: (comment) @string)

(command_name) @function

; Structure: the command a keyword, the title a heading.
(part command: _ @keyword text: (curly_group) @markup.heading)
(chapter command: _ @keyword text: (curly_group) @markup.heading)
(section command: _ @keyword text: (curly_group) @markup.heading)
(subsection command: _ @keyword text: (curly_group) @markup.heading)
(subsubsection command: _ @keyword text: (curly_group) @markup.heading)
(paragraph command: _ @keyword text: (curly_group) @markup.heading)
(subparagraph command: _ @keyword text: (curly_group) @markup.heading)
(title_declaration command: _ @keyword text: (curly_group) @markup.heading)
(caption command: _ @keyword)
(author_declaration command: _ @keyword)

; Environments.
(begin command: _ @keyword name: (curly_group_text) @type)
(end command: _ @keyword name: (curly_group_text) @type)

; What is brought in.
(class_include command: _ @keyword path: (curly_group_path) @type)
(package_include command: _ @keyword paths: (curly_group_path_list) @type)
(latex_include command: _ @keyword path: (curly_group_path) @string)
(graphics_include command: _ @keyword path: (curly_group_path) @string)
(new_command_definition command: _ @keyword)

; Labels, references and citations.
(label_definition command: _ @keyword name: (curly_group_label) @label)
(label_reference command: _ @keyword names: (curly_group_label_list) @label)
(label_reference_range command: _ @keyword)
(citation command: _ @keyword keys: (curly_group_text_list) @label)
(key_value_pair key: (text) @property)

; Mathematics.
(inline_formula) @string
(displayed_equation) @string
(math_environment) @string
