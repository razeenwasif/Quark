//! What a new project starts from: an article, a paper, a report, a thesis
//! in several files, slides and a CV, each a few files held in Quark
//! itself and written out with the project's title in them.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::project::wants;

/// One template: its files, as `(path, text)`, `main.tex` first.
struct Template {
    id: &'static str,
    name: &'static str,
    about: &'static str,
    files: &'static [(&'static str, &'static str)],
}

/// A template's file, held in Quark.
macro_rules! held {
    ($template:literal, $file:literal) => {
        (
            $file,
            include_str!(concat!("../templates/", $template, "/", $file)),
        )
    };
}

const TEMPLATES: &[Template] = &[
    Template {
        id: "article",
        name: "Article",
        about: "A short paper or essay: a title, an abstract, sections and references.",
        files: &[
            held!("article", "main.tex"),
            held!("article", "references.bib"),
        ],
    },
    Template {
        id: "paper",
        name: "Research paper",
        about: "Theorems and proofs, a table, and references by number with natbib.",
        files: &[held!("paper", "main.tex"), held!("paper", "references.bib")],
    },
    Template {
        id: "report",
        name: "Report",
        about: "Chapters, with a table of contents.",
        files: &[held!("report", "main.tex")],
    },
    Template {
        id: "thesis",
        name: "Thesis",
        about: "A book of several files, a chapter in each, typeset from main.tex.",
        files: &[
            held!("thesis", "main.tex"),
            held!("thesis", "chapters/introduction.tex"),
            held!("thesis", "chapters/background.tex"),
            held!("thesis", "chapters/conclusion.tex"),
            held!("thesis", "references.bib"),
        ],
    },
    Template {
        id: "slides",
        name: "Slides",
        about: "A Beamer presentation: a title page, an outline, and frames.",
        files: &[held!("slides", "main.tex")],
    },
    Template {
        id: "cv",
        name: "CV",
        about: "One page: education, experience and skills.",
        files: &[held!("cv", "main.tex")],
    },
];

/// A template, for the window's gallery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateView {
    pub id: String,
    pub name: String,
    pub about: String,
    /// The files it makes, `main.tex` first.
    pub files: Vec<String>,
    /// The classes and packages it asks TeX for: `article.cls`, `amsmath.sty`.
    pub wants: Vec<String>,
}

/// The templates there are, in the gallery's order.
pub fn templates() -> Vec<TemplateView> {
    TEMPLATES
        .iter()
        .map(|t| TemplateView {
            id: t.id.into(),
            name: t.name.into(),
            about: t.about.into(),
            files: t.files.iter().map(|(path, _)| path.to_string()).collect(),
            wants: t
                .files
                .iter()
                .filter(|(path, _)| path.ends_with(".tex"))
                .flat_map(|(_, text)| wants(text))
                .fold(Vec::new(), |mut all, file| {
                    if !all.contains(&file) {
                        all.push(file);
                    }
                    all
                }),
        })
        .collect()
}

/// What writing a template made: the folder, its main file, and the files
/// written, as the folder names them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Made {
    pub folder: PathBuf,
    pub main: PathBuf,
    pub wrote: Vec<String>,
}

/// Text as TeX would set it: its special characters escaped.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' | '%' | '$' | '#' | '_' | '{' | '}' => {
                out.push('\\');
                out.push(c);
            }
            '~' => out.push_str("\\textasciitilde{}"),
            '^' => out.push_str("\\textasciicircum{}"),
            '\\' => out.push_str("\\textbackslash{}"),
            c => out.push(c),
        }
    }
    out
}

/// A template's files written into `folder`, which must not hold any of
/// them, with `title` in them.
fn write(template: &Template, folder: &Path, title: &str) -> Result<Made, String> {
    let title = escape(title.trim());
    let mut wrote = Vec::new();
    for (path, text) in template.files {
        let target = folder.join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        let text = text.replace("{{title}}", &title);
        // Never over a file that is there: `create_new` says so instead.
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
            .map_err(|e| format!("{}: {e}", target.display()))?;
        std::io::Write::write_all(&mut file, text.as_bytes())
            .map_err(|e| format!("{}: {e}", target.display()))?;
        wrote.push(path.to_string());
    }
    Ok(Made {
        folder: folder.to_path_buf(),
        main: folder.join(template.files[0].0),
        wrote,
    })
}

fn find(id: &str) -> Result<&'static Template, String> {
    TEMPLATES
        .iter()
        .find(|t| t.id == id)
        .ok_or_else(|| format!("There is no template called {id}."))
}

/// A new project called `name`, in a folder of that name in `parent`, from
/// the template `id`. The folder must be new or empty; `parent` is made if
/// the folder it would be in is there.
pub fn create(id: &str, parent: &Path, name: &str) -> Result<Made, String> {
    let template = find(id)?;
    let name = name.trim();
    if name.is_empty() {
        return Err("A new project needs a name.".into());
    }
    if name.starts_with('.') || name.contains(['/', '\\', ':']) {
        return Err(format!(
            "{name}: a project's name is a folder's, without / or a leading dot."
        ));
    }
    if !parent.is_dir() {
        if parent.exists() {
            return Err(format!("{}: not a folder", parent.display()));
        }
        match parent.parent() {
            Some(above) if above.is_dir() => {
                std::fs::create_dir(parent).map_err(|e| format!("{}: {e}", parent.display()))?
            }
            _ => {
                return Err(format!(
                    "{}: no such folder",
                    parent.parent().unwrap_or(parent).display()
                ))
            }
        }
    }
    let folder = parent.join(name);
    if folder.exists() {
        let empty = std::fs::read_dir(&folder)
            .map(|mut d| d.next().is_none())
            .unwrap_or(false);
        if !empty {
            return Err(format!(
                "{} is not empty: a new project needs a folder of its own.",
                folder.display()
            ));
        }
    } else {
        std::fs::create_dir(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    }
    write(template, &folder, name)
}

/// The template `id` added to the project at `root`: at its top when none
/// of the template's files are there yet, else in a folder of its own —
/// `slides/`, `slides-2/` — so nothing of the project's is written over.
pub fn add(id: &str, root: &Path) -> Result<Made, String> {
    let template = find(id)?;
    let clashes = |folder: &Path| {
        template
            .files
            .iter()
            .any(|(path, _)| folder.join(path).exists())
    };
    let folder = if !clashes(root) {
        root.to_path_buf()
    } else {
        (1..100)
            .map(|n| {
                root.join(if n == 1 {
                    id.to_string()
                } else {
                    format!("{id}-{n}")
                })
            })
            .find(|folder| !folder.exists())
            .ok_or_else(|| format!("{}: no room for another {id}", root.display()))?
    };
    let title = root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("Untitled")
        .to_string();
    write(template, &folder, &title)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("quark-tex-templates-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn every_template_begins_a_document_says_what_it_wants_and_brings_in_only_files_it_has() {
        let all = templates();
        assert_eq!(
            all.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            ["article", "paper", "report", "thesis", "slides", "cv"]
        );
        for t in TEMPLATES {
            let (main, text) = t.files[0];
            assert_eq!(main, "main.tex");
            assert!(
                crate::project::is_main(text),
                "{} does not begin a document",
                t.id
            );
            for included in crate::project::includes(text) {
                assert!(
                    t.files.iter().any(|(path, _)| *path == included),
                    "{} brings in {included}, which it does not have",
                    t.id
                );
            }
            if text.contains("\\bibliography{references}") {
                assert!(
                    t.files.iter().any(|(path, _)| *path == "references.bib"),
                    "{} cites without a .bib",
                    t.id
                );
            }
        }
        let paper = all.iter().find(|t| t.id == "paper").unwrap();
        assert!(
            [
                "article.cls",
                "amsthm.sty",
                "booktabs.sty",
                "natbib.sty",
                "geometry.sty"
            ]
            .iter()
            .all(|f| paper.wants.iter().any(|w| w == f)),
            "{:?}",
            paper.wants
        );
        let slides = all.iter().find(|t| t.id == "slides").unwrap();
        assert!(
            slides.wants.contains(&"beamer.cls".to_string())
                && slides.wants.contains(&"beamerthemedefault.sty".to_string()),
            "{:?}",
            slides.wants
        );
    }

    #[test]
    fn a_new_project_is_made_in_a_new_or_empty_folder_with_its_title_escaped() {
        let parent = scratch("create");
        let made = create("thesis", &parent.join("projects"), "Q&A on 100% of it").unwrap();
        assert_eq!(made.folder, parent.join("projects/Q&A on 100% of it"));
        assert_eq!(made.main, made.folder.join("main.tex"));
        assert_eq!(made.wrote.len(), 5);
        assert!(made.folder.join("chapters/background.tex").is_file());
        let main = std::fs::read_to_string(&made.main).unwrap();
        assert!(main.contains("\\title{Q\\&A on 100\\% of it}"), "{main}");
        assert!(!main.contains("{{title}}"));

        let again = create("article", &parent.join("projects"), "Q&A on 100% of it").unwrap_err();
        assert!(again.contains("not empty"), "{again}");
        assert!(create("article", &parent, "../out")
            .unwrap_err()
            .contains("without /"));
        assert!(create("article", &parent, "  ")
            .unwrap_err()
            .contains("needs a name"));
        assert!(create("article", &parent.join("no/such"), "x")
            .unwrap_err()
            .contains("no such folder"));
        assert!(create("nothing", &parent, "x")
            .unwrap_err()
            .contains("no template"));
        std::fs::create_dir(parent.join("empty")).unwrap();
        assert!(
            create("cv", &parent, "empty").is_ok(),
            "an empty folder will do"
        );
        let _ = std::fs::remove_dir_all(parent);
    }

    #[test]
    fn a_template_added_to_a_project_writes_over_nothing() {
        let root = scratch("add");
        std::fs::write(root.join("notes.txt"), "mine").unwrap();
        let first = add("article", &root).unwrap();
        assert_eq!(first.folder, root, "nothing clashes: at the top");
        let second = add("article", &root).unwrap();
        assert_eq!(second.folder, root.join("article"));
        let third = add("article", &root).unwrap();
        assert_eq!(third.folder, root.join("article-2"));
        assert_eq!(
            std::fs::read_to_string(root.join("notes.txt")).unwrap(),
            "mine"
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
