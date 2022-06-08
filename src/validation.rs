use std::rc::Rc;

use crate::prepare_queries::PreparedField;
use crate::read_queries::ModuleInfo;
use crate::utils::has_duplicate;

use crate::parser::{
    BindParameter, NullableIdent, Parsed, ParsedModule, Query, QueryAnnotation, QueryDataStructure,
    TypeAnnotationListItem,
};

#[derive(Debug)]
pub(crate) struct ValidatedModule {
    pub(crate) info: Rc<ModuleInfo>,
    pub(crate) param_types: Vec<TypeAnnotationListItem>,
    pub(crate) row_types: Vec<TypeAnnotationListItem>,
    pub(crate) _db_types: Vec<TypeAnnotationListItem>,
    pub(crate) queries: Vec<ValidatedQuery>,
}

#[derive(Debug)]
pub(crate) enum ValidatedQuery {
    PgCompatible {
        name: Parsed<String>,
        params: Vec<Parsed<NullableIdent>>,
        row: Vec<Parsed<NullableIdent>>,
        sql_str: String,
    },
    Extended {
        name: Parsed<String>,
        params: QueryDataStructure,
        bind_params: Vec<Parsed<String>>,
        row: QueryDataStructure,
        sql_str: String,
    },
}
impl ValidatedQuery {
    pub(crate) fn name(&self) -> &Parsed<String> {
        match self {
            ValidatedQuery::PgCompatible { name, .. } => name,
            ValidatedQuery::Extended { name, .. } => name,
        }
    }
    pub(crate) fn sql_str(&self) -> &String {
        match self {
            ValidatedQuery::PgCompatible { sql_str, .. } => sql_str,
            ValidatedQuery::Extended { sql_str, .. } => sql_str,
        }
    }
}

use error::{Error, ErrorVariant};
use postgres::Column;
use postgres_types::Type;

pub(crate) fn ambiguous_bind_param(
    info: &Rc<ModuleInfo>,
    bind_params: &[Parsed<BindParameter>],
) -> Result<bool, Error> {
    // We're taking the first bind parameter as the gauge of what syntax is used.
    // This is pretty ad-hoc, it might worthwhile to add an explicit syntax marker (or smth similar).
    let syntax_is_extended = bind_params
        .get(0)
        .map(|bind_param| matches!(bind_param.value, BindParameter::Extended(_)))
        .unwrap_or(true);
    for bind_param in &bind_params[0..] {
        let bind_param_is_extended = matches!(bind_param.value, BindParameter::Extended(_));
        if syntax_is_extended ^ bind_param_is_extended {
            return Err(Error {
                err: ErrorVariant::AmbiguousBindParam {
                    pos: bind_param.start,
                },
                info: info.clone(),
            });
        }
    }

    Ok(syntax_is_extended)
}

pub(crate) fn duplicate_nullable_ident(
    info: &Rc<ModuleInfo>,
    idents: &[Parsed<NullableIdent>],
) -> Result<(), Error> {
    if let Some(dup) = has_duplicate(idents, |p| p.value.name()) {
        return Err(Error {
            err: ErrorVariant::DuplicateCol { pos: dup.start },
            info: info.clone(),
        });
    }
    Ok(())
}

type ParsedNullableIdents = Vec<Parsed<NullableIdent>>;
pub(crate) fn named_struct_in_pg_query(
    info: &Rc<ModuleInfo>,
    annotation: QueryAnnotation,
) -> Result<(ParsedNullableIdents, ParsedNullableIdents), Error> {
    if let QueryDataStructure::Named(name) = annotation.param {
        return Err(Error {
            err: ErrorVariant::NamedStructInPgQuery { pos: name.start },
            info: info.clone(),
        });
    };
    if let QueryDataStructure::Named(name) = annotation.row {
        return Err(Error {
            err: ErrorVariant::NamedStructInPgQuery { pos: name.start },
            info: info.clone(),
        });
    };

    let param = match annotation.param {
        QueryDataStructure::Implicit { idents } => idents,
        QueryDataStructure::Named(_) => unreachable!(),
    };
    let row = match annotation.row {
        QueryDataStructure::Implicit { idents } => idents,
        QueryDataStructure::Named(_) => unreachable!(),
    };
    Ok((param, row))
}

pub(crate) fn more_bind_params_than_params(
    info: &Rc<ModuleInfo>,
    params: &[Parsed<NullableIdent>],
    deduped_bind_params: &[Parsed<i16>],
) -> Result<(), Error> {
    let params_len = params.len();
    if let Some(bind_param) = deduped_bind_params
        .iter()
        .find(|bind_param| bind_param.value as usize > params_len)
    {
        return Err(Error {
            err: ErrorVariant::MoreBindParamsThanParams {
                nb_params: params.len(),
                pos: bind_param.start,
            },
            info: info.clone(),
        });
    }
    Ok(())
}

pub(crate) fn unused_param(
    info: &Rc<ModuleInfo>,
    params: &[Parsed<NullableIdent>],
    bind_params: &[Parsed<i16>],
) -> Result<(), Error> {
    if let Some((index, p)) = params.iter().enumerate().find(|(index, _)| {
        !bind_params
            .iter()
            .any(|bind_index| bind_index.value as usize == *index + 1)
    }) {
        return Err(Error {
            err: ErrorVariant::UnusedParam {
                index: index + 1,
                pos: p.start,
            },
            info: info.clone(),
        });
    };
    Ok(())
}

pub(crate) fn i16_index(
    info: &Rc<ModuleInfo>,
    Parsed { start, end, value }: Parsed<BindParameter>,
) -> Result<Parsed<i16>, Error> {
    let usize_index = match value {
        BindParameter::PgCompatible(index) => index,
        BindParameter::Extended(_) => unreachable!(),
    };
    // Check that the index can be parsed as a i16 (required by postgres wire protocol)
    let i16_index = i16::try_from(usize_index).map_err(|_| Error {
        err: ErrorVariant::InvalidI16Index { pos: start },
        info: info.clone(),
    })?;

    // Check that the index is also non-zero (postgres bind params are 1-indexed)
    if i16_index == 0 {
        return Err(Error {
            err: ErrorVariant::InvalidI16Index { pos: start },
            info: info.clone(),
        });
    };

    Ok(Parsed {
        start,
        end,
        value: i16_index,
    })
}

pub(crate) fn query_name_already_used(
    info: &Rc<ModuleInfo>,
    queries: &[Query],
) -> Result<(), Error> {
    for (i, query) in queries.iter().enumerate() {
        if let Some((_, q)) = queries
            .iter()
            .enumerate()
            .find(|(j, q)| *j != i && q.annotation.name == query.annotation.name)
        {
            return Err(Error {
                err: ErrorVariant::QueryNameAlreadyUsed {
                    name1: query.annotation.name.clone(),
                    name2: q.annotation.name.clone(),
                },
                info: info.clone(),
            });
        }
    }

    has_duplicate(queries.iter(), |q| &q.annotation.name);

    Ok(())
}

pub(crate) fn nullable_column_name(
    info: &Rc<ModuleInfo>,
    nullable_col: &Parsed<NullableIdent>,
    stmt_cols: &[Column],
) -> Result<(), Error> {
    // If none of the row's columns match the nullable column
    if stmt_cols
        .iter()
        .any(|row_col| row_col.name() == nullable_col.value.name())
    {
        Ok(())
    } else {
        Err(Error {
            err: ErrorVariant::InvalidNullableColumnName {
                nullable_col: nullable_col.clone(),
            },
            info: info.clone(),
        })
    }
}

pub(crate) fn nullable_param_name(
    info: &Rc<ModuleInfo>,
    nullable_col: &Parsed<NullableIdent>,
    params: &[(Parsed<String>, Type)],
) -> Result<(), Error> {
    // If none of the row's columns match the nullable column
    if params
        .iter()
        .any(|(name, _)| name.value == nullable_col.value.name())
    {
        Ok(())
    } else {
        Err(Error {
            err: ErrorVariant::InvalidNullableColumnName {
                nullable_col: nullable_col.clone(),
            },
            info: info.clone(),
        })
    }
}

pub(crate) fn named_struct_field(
    info: &Rc<ModuleInfo>,
    name: &Parsed<String>,
    prev_fields: &[PreparedField],
    fields: &[PreparedField],
) -> Result<(), Error> {
    // If the row doesn't contain the same fields as a previously
    // registered row with the same name...
    if prev_fields.len() == fields.len() || prev_fields.iter().all(|f| fields.contains(f)) {
        Ok(())
    } else {
        Err(Error {
            err: ErrorVariant::NamedStructInvalidFields {
                expected: prev_fields.to_owned(),
                actual: fields.to_owned(),
                name: name.clone(),
            },
            info: info.clone(),
        })
    }
}

pub(crate) fn unknown_named_struct(
    info: &Rc<ModuleInfo>,
    name: &Parsed<String>,
    types: &[TypeAnnotationListItem],
) -> Result<Vec<Parsed<NullableIdent>>, Error> {
    if let Some(x) = types.iter().find(|x| &x.name == name) {
        Ok(x.fields.clone())
    } else {
        Err(Error {
            err: ErrorVariant::UnknownNamedStruct { pos: name.start },
            info: info.clone(),
        })
    }
}

pub(crate) fn validate_query(info: &Rc<ModuleInfo>, query: Query) -> Result<ValidatedQuery, Error> {
    if let QueryDataStructure::Implicit { idents } = &query.annotation.param {
        duplicate_nullable_ident(info, idents)?;
    };
    if let QueryDataStructure::Implicit { idents } = &query.annotation.row {
        duplicate_nullable_ident(info, idents)?;
    };
    let name = query.annotation.name.clone();
    let is_extended_syntax = ambiguous_bind_param(info, &query.sql.bind_params)?;
    let validated_query = if is_extended_syntax {
        let mut bind_params = query
            .sql
            .bind_params
            .iter()
            .map(|bind_param| {
                bind_param.map(|bind_param| match bind_param {
                    BindParameter::Extended(e) => e.clone(),
                    BindParameter::PgCompatible(_) => {
                        unreachable!()
                    }
                })
            })
            .collect::<Vec<Parsed<String>>>();
        bind_params.sort();
        bind_params.dedup();

        let sql_str = query.sql.normalize_sql(query.sql_start);
        ValidatedQuery::Extended {
            name: query.annotation.name,
            params: query.annotation.param,
            bind_params,
            row: query.annotation.row,
            sql_str,
        }
    } else {
        let bind_params = &query
            .sql
            .bind_params
            .into_iter()
            .map(|bind_param| i16_index(info, bind_param))
            .collect::<Result<Vec<Parsed<i16>>, Error>>()?;
        let mut deduped_bind_params = bind_params.clone();
        deduped_bind_params.sort();
        deduped_bind_params.dedup();

        let (params, row) = named_struct_in_pg_query(info, query.annotation)?;

        more_bind_params_than_params(info, &params, &deduped_bind_params)?;
        unused_param(info, &params, bind_params)?;

        ValidatedQuery::PgCompatible {
            name,
            params,
            row,
            sql_str: query.sql.sql_str,
        }
    };

    Ok(validated_query)
}

pub(crate) fn validate_module(
    info: Rc<ModuleInfo>,
    module: ParsedModule,
) -> Result<ValidatedModule, Error> {
    query_name_already_used(&info, &module.queries)?;
    for ty in module
        .param_types
        .iter()
        .chain(module.row_types.iter())
        .chain(module.db_types.iter())
    {
        duplicate_nullable_ident(&info, &ty.fields)?;
    }
    let mut validated_queries = Vec::new();
    for query in module.queries {
        validated_queries.push(validate_query(&info, query)?);
    }
    Ok(ValidatedModule {
        info,
        param_types: module.param_types,
        row_types: module.row_types,
        _db_types: module.db_types,
        queries: validated_queries,
    })
}

pub mod error {
    use std::{fmt::Display, rc::Rc};

    use crate::{
        parser::{NullableIdent, Parsed},
        prepare_queries::PreparedField,
        read_queries::ModuleInfo,
        utils::compute_line,
    };

    #[derive(Debug)]
    pub enum ErrorVariant {
        AmbiguousBindParam {
            pos: usize,
        },
        InvalidI16Index {
            pos: usize,
        },
        DuplicateCol {
            pos: usize,
        },
        MoreBindParamsThanParams {
            nb_params: usize,
            pos: usize,
        },
        UnusedParam {
            index: usize,
            pos: usize,
        },
        InvalidNullableColumnName {
            nullable_col: Parsed<NullableIdent>,
        },
        NamedStructInvalidFields {
            expected: Vec<PreparedField>,
            actual: Vec<PreparedField>,
            name: Parsed<String>,
        },
        QueryNameAlreadyUsed {
            name1: Parsed<String>,
            name2: Parsed<String>,
        },
        NamedStructInPgQuery {
            pos: usize,
        },
        UnknownNamedStruct {
            pos: usize,
        },
    }

    #[derive(Debug)]
    pub struct Error {
        pub(crate) err: ErrorVariant,
        pub(crate) info: Rc<ModuleInfo>,
    }

    impl Display for Error {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            let head = format!(
                "Error while validating queries [path: \"{}\"]:\n",
                self.info.path
            );
            match &self.err {
                ErrorVariant::InvalidI16Index { pos } => {
                    let msg = ["Index must be between 1 and 32767."];
                    write!(f, "{head}{}", format_err(&self.info, *pos, &msg))
                }
                ErrorVariant::DuplicateCol { pos } => {
                    let msg = ["Column name is already used."];
                    write!(f, "{head}{}", format_err(&self.info, *pos, &msg))
                }
                ErrorVariant::MoreBindParamsThanParams { pos, nb_params } => {
                    let msg = format!(
                        "Index is higher than the number of parameters supplied ({nb_params})."
                    );
                    write!(f, "{head}{}", format_err(&self.info, *pos, &[&msg]))
                }
                ErrorVariant::UnusedParam { pos, index } => {
                    let msg = format!("Parameter `${index}` is never used in the query.");
                    write!(f, "{head}{}", format_err(&self.info, *pos, &[&msg]))
                }
                ErrorVariant::InvalidNullableColumnName { nullable_col } => {
                    let name = nullable_col.value.name();
                    let msg = format!("No column named `{name}` found for this query.");
                    write!(
                        f,
                        "{head}{}",
                        format_err(&self.info, nullable_col.start, &[&msg])
                    )
                }
                // Move into another module
                ErrorVariant::NamedStructInvalidFields {
                    name,
                    expected,
                    actual,
                } => {
                    let msg1 = format!("This query's named row struct `{}` has already been used, but the fields don't match.", name.value);
                    let msg2 = format!("Expected fields: {expected:#?}");
                    let msg3 = format!("Got fields: {actual:#?}");
                    write!(
                        f,
                        "{head}{}",
                        format_err(&self.info, name.start, &[&msg1, &msg2, &msg3])
                    )
                }
                ErrorVariant::QueryNameAlreadyUsed { name1, name2 } => {
                    let msg1 = format!("A query named `{}` already exists.", name1.value);
                    let msg2 = format!("Query `{}` first defined here.", name2.value);
                    write!(
                        f,
                        "{head}{}\n{}",
                        format_err(&self.info, name1.start, &[&msg1]),
                        format_err(&self.info, name2.start, &[&msg2])
                    )
                }
                ErrorVariant::AmbiguousBindParam { pos } => {
                    let msg = [
                                "Cannot mix bind parameter syntaxes in the same query.", 
                                "Please use either named (`:named_ident`) or indexed (`$n`) bind parameters, but not both."
                            ];
                    write!(f, "{head}{}", format_err(&self.info, *pos, &msg))
                }
                ErrorVariant::NamedStructInPgQuery { pos } => {
                    let msg = ["Named query structs are not allowed when using the PostgreSQL-compatible syntax.",
                    "Use anonymous structs instead, or use the extended query syntax."];
                    write!(f, "{head}{}", format_err(&self.info, *pos, &msg))
                }
                ErrorVariant::UnknownNamedStruct { pos } => {
                    let msg = "Unknown named struct. Named structs must be registered using type annotations.";
                    write!(f, "{head}{}", format_err(&self.info, *pos, &[msg]))
                }
            }
        }
    }
    impl std::error::Error for Error {}

    fn format_err(info: &ModuleInfo, pos: usize, messages: &[&str]) -> String {
        let msg = messages.join("\n  = ");
        let (col, line, line_str) = compute_line(&info.content, pos);
        let cursor = format!("{}^---", " ".repeat(col - 1));
        format!(" --> {line}:{col}\n  | \n  | {line_str}\n  | {cursor}\n  | \n  = {msg}")
    }
}
