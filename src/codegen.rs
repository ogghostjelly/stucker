use std::{
    collections::HashMap,
    fmt,
    io::{self, Cursor, Seek},
};

use crate::{
    ast::{
        Abi, DefAssignment, Expression, ExpressionType, ForStatement, Function, GlobalValue,
        Number, NumberType, SetAssignment, Statement,
    },
    write_asm, write_rdat,
};

pub struct Codegen<W: io::Write> {
    nasm: Nasm<W>,
    def: DefinitionTable,
}

#[derive(Default)]
pub struct DefinitionTable {
    fn_table: HashMap<String, DefinedFunction>,
    struct_table: HashMap<String, DefinedStruct>,
}

pub enum DefinedFunction {
    Stucker(StuckerFunction),
    C(CFunction),
}

pub struct StuckerFunction {
    return_type: ExpressionType,
    params: Vec<ExpressionType>,
}

pub struct CFunction {
    return_type: Option<NumberType>,
    params: Vec<NumberType>,
    is_variadic: bool,
}

pub struct DefinedStruct {
    fields: Vec<(ExpressionType, String)>,
}

impl<W: io::Write> Codegen<W> {
    pub fn new(writer: W) -> Self {
        Self {
            nasm: Nasm::new(writer),
            def: DefinitionTable::default(),
        }
    }

    pub fn init(&mut self) -> Result<()> {
        write_asm!(self.nasm, "global main")?;
        write_asm!(self.nasm, "section .text")?;

        self.nasm.raw_label("main")?;
        write_asm!(self.nasm, "mov rbp, 0")?;

        let (_, idx) = FunctionCodegen::new(&mut self.nasm, &mut VarTable::default(), &self.def)
            .codegen_call(
                &DefinedFunction::Stucker(StuckerFunction {
                    return_type: ExpressionType::Number(NumberType::I32),
                    params: vec![],
                }),
                "main".into(),
                vec![],
            )?;

        self.nasm.idx2addr("rbx", &idx)?;
        write_asm!(self.nasm, "mov ebx, [rbx+8]")?;

        write_asm!(self.nasm, "mov eax, 1")?;
        write_asm!(self.nasm, "int 0x80")?;

        self.codegen_stdlib()?;

        Ok(())
    }

    pub fn deinit(&mut self) -> Result<()> {
        self.nasm.finalize()?;
        Ok(())
    }

    pub fn codegen(&mut self, name: String, value: GlobalValue) -> Result<()> {
        match value {
            GlobalValue::Function(function) => {
                let Function {
                    return_type,
                    params,
                    body,
                    abi,
                    is_variadic,
                } = function;

                if name == "main" {
                    if return_type != ExpressionType::Number(NumberType::I32) {
                        return Err(Error::MainInvalidSignature);
                    }
                    if !params.is_empty() {
                        return Err(Error::MainInvalidSignature);
                    }
                    if is_variadic {
                        return Err(Error::MainInvalidSignature);
                    }
                }

                for (param_type, _) in &params {
                    if let ExpressionType::Struct(struc) = param_type {
                        _ = self.def.get_struct(struc)?;
                    }
                }

                match abi {
                    Abi::C => {
                        if body.is_some() {
                            return Err(Error::ExternFunctionBody);
                        }

                        let return_type = match return_type {
                            ExpressionType::Number(numty) => Some(numty),
                            ExpressionType::Void => None,
                            _ => return Err(Error::ExternCInvalidParameter),
                        };

                        let params = params
                            .iter()
                            .map(|(x, _)| match x {
                                ExpressionType::Number(numty) => Ok(*numty),
                                _ => Err(Error::ExternCInvalidParameter),
                            })
                            .collect::<Result<Vec<_>>>()?;

                        self.def.fn_table.insert(
                            name.clone(),
                            DefinedFunction::C(CFunction {
                                return_type,
                                params,
                                is_variadic,
                            }),
                        );

                        writeln!(self.nasm.writer, "\nextern {name}")?;
                        _ = self.nasm.global_label(&name)?;
                        write_asm!(self.nasm, "jmp {name}")?;
                    }
                    Abi::Stucker => {
                        if is_variadic {
                            return Err(Error::VariadicStuckerFunction);
                        }

                        self.def.fn_table.insert(
                            name.clone(),
                            DefinedFunction::Stucker(StuckerFunction {
                                return_type: return_type.clone(),
                                params: params.iter().map(|(x, _)| x.clone()).collect(),
                            }),
                        );

                        if let Some(body) = body {
                            self.codegen_prologue(&return_type, &name)?;

                            let mut var = VarTable::default();

                            for (param_type, param_name) in params {
                                let idx = self.nasm.push_supress();
                                var.data.insert(param_name, (param_type.clone(), idx));
                            }

                            for stmt in body {
                                self.codegen_stmt(&return_type, &mut var, stmt)?;
                            }

                            self.nasm.ret(&return_type)?;
                        }
                    }
                }

                Ok(())
            }
            GlobalValue::Struct(struc) => {
                let mut fields = Vec::with_capacity(struc.0.len());

                for (field_type, field_name) in struc.0 {
                    fields.push((field_type, field_name));
                }

                self.def.struct_table.insert(name, DefinedStruct { fields });
                Ok(())
            }
        }
    }

    pub fn codegen_prologue(&mut self, return_type: &ExpressionType, name: &str) -> Result<()> {
        self.nasm.new_stack()?;
        self.nasm.global_label(name)?;

        if *return_type != ExpressionType::Void {
            let _ret_idx = self.nasm.push_supress();
        }
        let _rax_idx = self.nasm.push_supress();

        Ok(())
    }

    pub fn codegen_stdlib(&mut self) -> Result<()> {
        self.nasm.new_stack()?;
        writeln!(self.nasm.writer)?;
        self.nasm.raw_label("copy_addr")?;
        self.nasm.copy_addr_impl()?;
        write_asm!(self.nasm, "jmp rax")?;

        self.nasm.new_stack()?;
        writeln!(self.nasm.writer)?;
        self.nasm.raw_label("ref2addr")?;
        self.nasm.ref2addr_impl()?;
        write_asm!(self.nasm, "jmp rax")?;

        self.codegen_builtin(
            "resize".into(),
            ExpressionType::Void,
            vec![
                ExpressionType::Ref(Box::new(ExpressionType::Void)),
                ExpressionType::Number(NumberType::U64),
            ],
            |c, p| {
                let [ptr_idx, size_idx] = p.try_into().unwrap();

                c.nasm.idx2addr("rbx", &ptr_idx)?;
                c.nasm.ref2addr()?;
                write_asm!(c.nasm, "mov rbx, rsi")?;

                c.nasm.idx2addr("rdx", &size_idx)?;
                write_asm!(c.nasm, "mov rdx, [rdx+8]")?;
                write_asm!(c.nasm, "add rdx, 8")?; // account for 8-byte size tag

                c.nasm.resize_impl()?;

                Ok(())
            },
        )?;

        Ok(())
    }

    pub fn codegen_builtin(
        &mut self,
        name: String,
        return_type: ExpressionType,
        params: Vec<ExpressionType>,
        body: impl FnOnce(&mut Self, Vec<Index>) -> Result<()>,
    ) -> Result<()> {
        self.codegen_prologue(&return_type, &name)?;

        let mut param_idxs = Vec::with_capacity(params.len());
        for _ in 0..params.len() {
            param_idxs.push(self.nasm.push_supress());
        }

        self.def.fn_table.insert(
            name,
            DefinedFunction::Stucker(StuckerFunction {
                return_type: return_type.clone(),
                params,
            }),
        );

        body(self, param_idxs)?;

        self.nasm.ret(&return_type)?;

        Ok(())
    }

    pub fn codegen_stmt(
        &mut self,
        return_type: &ExpressionType,
        var: &mut VarTable,
        stmt: Statement,
    ) -> Result<()> {
        match stmt {
            Statement::DefAssign(assignment) => self.codegen_def_assignment(var, assignment),
            Statement::SetAssign(assignment) => self.codegen_set_assignment(var, assignment),
            Statement::Return(Some(expression)) => {
                let (expr_ty, expr_idx) = FunctionCodegen::new(&mut self.nasm, var, &self.def)
                    .codegen_expression(expression)?;

                if *return_type != expr_ty {
                    return Err(Error::TypeMismatch(return_type.clone(), expr_ty));
                }

                if *return_type != ExpressionType::Void {
                    self.nasm.idx2addr("rsi", &expr_idx)?;
                    // `0` is the index of the return value
                    self.nasm.idx2addr("rdi", &Index::new(0))?;
                    self.nasm.copy_addr()?;
                }

                Ok(())
            }
            Statement::Return(None) => self.nasm.ret(return_type),
            Statement::Expr(expression) => {
                _ = FunctionCodegen::new(&mut self.nasm, var, &self.def)
                    .codegen_expression(expression)?;
                Ok(())
            }
            Statement::Block(statements) => {
                let mut table = VarTable::new(Some(var.clone()));
                let ptr = self.nasm.stack_pointer;
                for stmt in statements {
                    self.codegen_stmt(return_type, &mut table, stmt)?;
                }
                self.nasm.pop_until(ptr)?;
                Ok(())
            }
            Statement::If(inn) => {
                let (cond, true_block, false_block) = *inn;

                self.codegen_cmp(var, cond)?;

                let ret = self.nasm.get_local_label_name("if_return");
                let false_label = self.nasm.get_local_label_name("if_false");

                write_asm!(self.nasm, "je {false_label}")?;

                let mut true_var = VarTable::new(Some(var.clone()));
                let ptr = self.nasm.stack_pointer;
                self.codegen_stmt(return_type, &mut true_var, true_block)?;
                self.nasm.pop_until(ptr)?;
                write_asm!(self.nasm, "jmp {ret}")?;

                self.nasm.raw_label(&false_label)?;
                if let Some(false_block) = false_block {
                    let mut false_var = VarTable::new(Some(var.clone()));
                    let ptr = self.nasm.stack_pointer;
                    self.codegen_stmt(return_type, &mut false_var, false_block)?;
                    self.nasm.pop_until(ptr)?;
                }

                self.nasm.raw_label(&ret)?;

                Ok(())
            }
            Statement::While(inn) => {
                let (cond, body) = *inn;

                let ret = self.nasm.get_local_label_name("while_return");

                let body_label = self.nasm.local_label("while_body")?;
                self.codegen_cmp(var, cond)?;
                write_asm!(self.nasm, "je {ret}")?;

                let mut body_var = VarTable::new(Some(var.clone()));
                let ptr = self.nasm.stack_pointer;
                self.codegen_stmt(return_type, &mut body_var, body)?;
                self.nasm.pop_until(ptr)?;

                write_asm!(self.nasm, "jmp {body_label}")?;
                self.nasm.raw_label(&ret)?;

                Ok(())
            }
            Statement::For(inn) => {
                let ForStatement {
                    init,
                    cond,
                    inc,
                    body,
                } = *inn;

                self.codegen_stmt(return_type, var, init)?;

                let ret = self.nasm.get_local_label_name("for_return");

                let body_label = self.nasm.local_label("for_body")?;
                self.codegen_cmp(var, cond)?;
                write_asm!(self.nasm, "je {ret}")?;

                {
                    let ptr = self.nasm.stack_pointer;
                    let mut body_var = VarTable::new(Some(var.clone()));
                    self.codegen_stmt(return_type, &mut body_var, body)?;
                    self.nasm.pop_until(ptr)?;
                    self.codegen_stmt(return_type, var, inc)?;
                    self.nasm.pop_until(ptr)?;
                }

                write_asm!(self.nasm, "jmp {body_label}")?;
                self.nasm.raw_label(&ret)?;

                Ok(())
            }
            Statement::Breakpoint => {
                write_asm!(self.nasm, "int3")?;
                Ok(())
            }
        }
    }

    pub fn codegen_cmp(&mut self, var: &mut VarTable, cond: Expression) -> Result<()> {
        let ptr = self.nasm.stack_pointer;
        let (expr_ty, expr_idx) =
            FunctionCodegen::new(&mut self.nasm, var, &self.def).codegen_expression(cond)?;
        FunctionCodegen::new(&mut self.nasm, var, &self.def).mov_num(
            (expr_ty.into_number()?, expr_idx),
            "bl",
            "bx",
            "ebx",
            "rbx",
            "rcx",
        )?;
        self.nasm.pop_until(ptr)?;

        write_asm!(self.nasm, "cmp rbx, 0")?;

        Ok(())
    }

    pub fn codegen_def_assignment(
        &mut self,
        var: &mut VarTable,
        assignment: DefAssignment,
    ) -> Result<()> {
        let DefAssignment {
            var_type,
            var_name,
            var_value,
        } = assignment;

        let idx = match var_value {
            Some(expr) => {
                let (expr_type, expr_idx) = FunctionCodegen::new(&mut self.nasm, var, &self.def)
                    .codegen_expression(expr)?;

                if expr_type != var_type {
                    return Err(Error::TypeMismatch(var_type, expr_type));
                }

                expr_idx
            }
            None => self.def.alloc(&mut self.nasm, &var_type)?,
        };

        var.data.insert(var_name, (var_type, idx));

        Ok(())
    }

    pub fn codegen_set_assignment(
        &mut self,
        var: &mut VarTable,
        assignment: SetAssignment,
    ) -> Result<()> {
        let SetAssignment {
            var_dest,
            var_src,
            deref,
        } = assignment;

        let (var_type, dest) =
            FunctionCodegen::new(&mut self.nasm, var, &self.def).codegen_expression(var_dest)?;
        let (expr_type, src) =
            FunctionCodegen::new(&mut self.nasm, var, &self.def).codegen_expression(var_src)?;

        let var_type = if deref {
            var_type.into_ref()?
        } else {
            var_type
        };

        if var_type != expr_type {
            return Err(Error::TypeMismatch(var_type, expr_type));
        }

        if deref {
            self.nasm.idx2addr("rbx", &dest)?;
            self.nasm.ref2addr()?;
            write_asm!(self.nasm, "mov rdi, rsi")?;
        } else {
            self.nasm.idx2addr("rdi", &dest)?;
        }
        self.nasm.idx2addr("rsi", &src)?;

        self.nasm.copy_addr()?;

        Ok(())
    }
}

impl DefinitionTable {
    pub fn alloc(
        &self,
        nasm: &mut Nasm<impl io::Write>,
        expr_ty: &ExpressionType,
    ) -> Result<Index> {
        match expr_ty {
            ExpressionType::Number(ty) => nasm.push(ty.size_bytes(), "number"),
            ExpressionType::Struct(k) => {
                let struc = self.get_struct(k)?;
                let mut size = 0;
                for (ty, _) in &struc.fields {
                    size += self.alloc_hidden(nasm, ty)?;
                }
                nasm.push_stag_val(size)
            }
            ExpressionType::Ref(_) => nasm.push(8, "ref"),
            ExpressionType::Array(_) => nasm.push(0, "array"),
            ExpressionType::Void => Err(Error::CannotAllocVoid),
        }
    }

    pub fn alloc_hidden(
        &self,
        nasm: &mut Nasm<impl io::Write>,
        expr_ty: &ExpressionType,
    ) -> Result<u16> {
        match expr_ty {
            ExpressionType::Number(ty) => nasm.push_hidden(ty.size_bytes()),
            ExpressionType::Struct(k) => {
                let struc = self.get_struct(k)?;
                let mut size = 0;
                for (ty, _) in &struc.fields {
                    size += self.alloc_hidden(nasm, ty)?;
                }
                nasm.push_stag_hidden(size)
            }
            ExpressionType::Ref(_) => nasm.push_hidden(8), // references are 64-bit
            ExpressionType::Array(_) => nasm.push_hidden(0),
            ExpressionType::Void => Err(Error::CannotAllocVoid),
        }
    }

    pub fn get_struct(&self, struc: &str) -> Result<&DefinedStruct> {
        match self.struct_table.get(struc) {
            Some(struc) => Ok(struc),
            None => Err(Error::UnknownType(struc.to_string())),
        }
    }
}

pub struct FunctionCodegen<'a, W: io::Write> {
    nasm: &'a mut Nasm<W>,
    var: &'a mut VarTable,
    def: &'a DefinitionTable,
}

impl<'a, W: io::Write> FunctionCodegen<'a, W> {
    pub fn new(nasm: &'a mut Nasm<W>, var: &'a mut VarTable, def: &'a DefinitionTable) -> Self {
        Self { nasm, var, def }
    }

    pub fn codegen_expression(&mut self, expr: Expression) -> Result<(ExpressionType, Index)> {
        match expr {
            Expression::Call(k, expressions) => match self.def.fn_table.get(&k) {
                Some(func) => self.codegen_call(func, k, expressions),
                None => Err(Error::UnknownFunction(k)),
            },
            Expression::Symbol(symbol) => self.var.get_symbol(symbol),
            Expression::Number(number) => self.codegen_number(number),
            Expression::BinOp(binop) => self.codegen_binop(*binop),
            Expression::Ref(expr) => {
                let (expr_type, expr_idx) = self.codegen_expression(*expr)?;
                let ref_idx = self.nasm.push_idx2ref(&expr_idx, "bx")?;
                Ok((ExpressionType::Ref(Box::new(expr_type)), ref_idx))
            }
            Expression::Deref(expr) => {
                let (ref_ty, ref_idx) = self.codegen_expression(*expr)?;
                let ref_value_ty = ref_ty.into_ref()?;

                self.nasm.idx2addr("rbx", &ref_idx)?;
                self.nasm.ref2addr()?;
                let expr_idx = self.nasm.push_copy_addr()?;

                Ok((ref_value_ty, expr_idx))
            }
            Expression::InitArray(inn) => {
                let (len, elem_ty) = *inn;

                let (index_ty, index_idx) = self.codegen_expression(len)?;
                let expected_ty = ExpressionType::Number(NumberType::U64);
                if expected_ty != index_ty {
                    return Err(Error::TypeMismatch(expected_ty, index_ty));
                }

                self.nasm.idx2addr("rcx", &index_idx)?;
                write_asm!(self.nasm, "mov rcx, [rcx+8]")?;
                write_asm!(self.nasm, "mov rdx, 8")?;

                let ret = self.nasm.get_local_label_name("init_array_return");

                let body = self.nasm.local_label("init_array_body")?;
                write_asm!(self.nasm, "cmp rcx, 0")?;
                write_asm!(self.nasm, "je {ret}")?;
                let size = self.def.alloc_hidden(self.nasm, &elem_ty)?;
                write_asm!(self.nasm, "add rdx, {size}")?;
                write_asm!(self.nasm, "sub rcx, 1")?;
                write_asm!(self.nasm, "jmp {body}")?;

                self.nasm.raw_label(&ret)?;

                let idx = self.nasm.push_stag_reg("rdx")?;

                Ok((ExpressionType::Array(Box::new(elem_ty)), idx))
            }
            Expression::As(inn) => {
                let (ty, inn) = *inn;
                let (expr_ty, idx) = self.codegen_expression(inn)?;
                let idx = match (expr_ty, &ty) {
                    (ExpressionType::Number(from), ExpressionType::Number(to)) => {
                        self.mov_num((from, idx), "b", "bx", "ebx", "rbx", "rcx")?;

                        let idx = match to {
                            NumberType::I8 => self.nasm.push(1, "as_i8"),
                            NumberType::I16 => self.nasm.push(2, "as_i16"),
                            NumberType::I32 => self.nasm.push(4, "as_i32"),
                            NumberType::I64 => self.nasm.push(8, "as_i64"),
                            NumberType::U8 => self.nasm.push(1, "as_u8"),
                            NumberType::U16 => self.nasm.push(2, "as_u16"),
                            NumberType::U32 => self.nasm.push(4, "as_u32"),
                            NumberType::U64 => self.nasm.push(8, "as_u64"),
                            NumberType::F32 => self.nasm.push(4, "as_f32"),
                            NumberType::F64 => self.nasm.push(8, "as_f64"),
                        }?;

                        match to {
                            NumberType::U8 | NumberType::I8 => {
                                write_asm!(self.nasm, "mov [rsp+8], b")
                            }
                            NumberType::U16 | NumberType::I16 => {
                                write_asm!(self.nasm, "mov [rsp+8], bx")
                            }
                            NumberType::F32 | NumberType::U32 | NumberType::I32 => {
                                write_asm!(self.nasm, "mov [rsp+8], ebx")
                            }
                            NumberType::F64 | NumberType::U64 | NumberType::I64 => {
                                write_asm!(self.nasm, "mov [rsp+8], rbx")
                            }
                        }?;

                        idx
                    }
                    _ => idx,
                };
                Ok((ty, idx))
            }
            Expression::FieldAccess(inn) => {
                let (expr, field_name) = *inn;

                let (struct_ty, struct_idx) = self.codegen_expression(expr)?;

                match struct_ty {
                    ExpressionType::Ref(struct_ty) => match *struct_ty {
                        ExpressionType::Struct(struct_name) => {
                            self.codegen_struct_ref_access(struct_name, struct_idx, field_name)
                        }
                        _ => Err(Error::CannotAccessStruct),
                    },
                    ExpressionType::Struct(struct_name) => {
                        self.codegen_struct_access(struct_name, struct_idx, field_name)
                    }
                    _ => Err(Error::CannotAccessStruct),
                }
            }
            Expression::ArrayAccess(inn) => {
                let (arr_expr, index_expr) = *inn;

                let (arr_ty, arr_idx) = self.codegen_expression(arr_expr)?;

                let (index_ty, index_idx) = self.codegen_expression(index_expr)?;
                let index_expected = ExpressionType::Number(NumberType::U16);
                if index_expected != index_ty {
                    return Err(Error::TypeMismatch(index_expected, index_ty));
                }

                match arr_ty {
                    ExpressionType::Ref(arr_ty) => match *arr_ty {
                        ExpressionType::Array(elem_ty) => {
                            self.codegen_array_ref_access(elem_ty, arr_idx, index_idx)
                        }
                        _ => Err(Error::CannotAccessArray),
                    },
                    ExpressionType::Array(_) => {
                        //self.codegen_array_access(*elem_ty, arr_idx, index_idx)
                        Err(Error::CannotAccessArray)
                    }
                    _ => Err(Error::CannotAccessArray),
                }
            }
            Expression::String(s) => {
                self.nasm.db_cstr(&s, "rbx")?;
                let idx = self.nasm.push_register("rbx", RegisterSize::S64)?;
                Ok((ExpressionType::Number(NumberType::U64), idx))
            }
        }
    }

    pub fn codegen_struct_access(
        &mut self,
        struct_name: String,
        struct_idx: Index,
        field_name: String,
    ) -> Result<(ExpressionType, Index)> {
        let struc = self.def.get_struct(&struct_name)?;
        let mut fields = (0u16..).zip(struc.fields.iter());
        let fields = fields.find(|(_, (_, f))| field_name == *f);

        match fields {
            Some((offset, (field_ty, _))) => {
                let mut struct_idx = struct_idx;
                let idx = struct_idx.valid_mut()?;
                idx.1.push(offset);
                Ok((field_ty.clone(), struct_idx))
            }
            None => Err(Error::UnknownField(struct_name, field_name)),
        }
    }

    pub fn codegen_struct_ref_access(
        &mut self,
        struct_name: String,
        struct_ref_idx: Index,
        field_name: String,
    ) -> Result<(ExpressionType, Index)> {
        let struc = self.def.get_struct(&struct_name)?;
        let mut fields = struc.fields.iter().enumerate();
        let fields = fields.find(|(_, (_, f))| field_name == *f);

        let (field_ty, offset) = match fields {
            Some((offset, (field_ty, _))) => (field_ty.clone(), offset as u16),
            None => return Err(Error::UnknownField(struct_name, field_name)),
        };

        self.nasm.idx2addr("rsi", &struct_ref_idx)?;
        let idx = self.nasm.push_copy_addr()?;

        write_asm!(self.nasm, "mov rbx, rsp")?;
        write_asm!(self.nasm, "add rbx, [rbx]")?;
        write_asm!(self.nasm, "sub rbx, 2")?;
        write_asm!(self.nasm, "add word [rbx], {offset}")?;

        Ok((ExpressionType::Ref(Box::new(field_ty)), idx))
    }

    /*pub fn codegen_array_access(
        &mut self,
        elem_ty: ExpressionType,
        arr_idx: Index,
        index_idx: Index,
    ) -> Result<(ExpressionType, Index)> {
        self.nasm.idx2addr("rsi", &arr_idx)?;
        write_asm!(self.nasm, "add rsi, 8")?;
        self.nasm.idx2addr("rcx", &index_idx)?;
        write_asm!(self.nasm, "mov cx, word [rcx+8]")?;

        let ret = self.nasm.get_local_label_name("array_access_return");

        let body = self.nasm.local_label("array_access_body")?;
        write_asm!(self.nasm, "cmp cx, 0")?;
        write_asm!(self.nasm, "je {ret}")?;
        write_asm!(self.nasm, "add rsi, [rsi]")?;
        write_asm!(self.nasm, "sub cx, 1")?;
        write_asm!(self.nasm, "jmp {body}")?;
        self.nasm.raw_label(&ret)?;

        let idx = self.nasm.push_copy_addr()?;
        Ok((elem_ty, idx))
    }*/

    pub fn codegen_array_ref_access(
        &mut self,
        elem_ty: Box<ExpressionType>,
        arr_ref_idx: Index,
        index_idx: Index,
    ) -> Result<(ExpressionType, Index)> {
        self.nasm.idx2addr("rsi", &arr_ref_idx)?;
        self.nasm.idx2addr("rcx", &index_idx)?;
        write_asm!(self.nasm, "mov cx, [rcx+8]")?;

        // push copy of rsi to stack with cx appended
        write_asm!(self.nasm, "sub rsp, 2")?;
        write_asm!(self.nasm, "mov [rsp], cx")?;
        let idx = self.nasm.push_copy_addr()?;
        write_asm!(self.nasm, "add qword [rsp], 2")?;

        Ok((ExpressionType::Ref(elem_ty), idx))
    }

    pub fn codegen_call(
        &mut self,
        func: &DefinedFunction,
        name: String,
        expressions: Vec<Expression>,
    ) -> Result<(ExpressionType, Index)> {
        let mut params = Vec::with_capacity(expressions.len());

        for expr in expressions {
            params.push(self.codegen_expression(expr)?);
        }

        let e = match func {
            DefinedFunction::Stucker(func) => self.codegen_call_stucker(func, name, params),
            DefinedFunction::C(func) => self.codegen_call_c(func, name, params),
        }?;

        Ok(e)
    }

    pub fn codegen_call_stucker(
        &mut self,
        func: &StuckerFunction,
        name: String,
        params: Vec<(ExpressionType, Index)>,
    ) -> Result<(ExpressionType, Index)> {
        if func.params.len() != params.len() {
            return Err(Error::ArityMismatch(func.params.len(), params.len()));
        }

        let ret_idx = if func.return_type != ExpressionType::Void {
            self.def.alloc(self.nasm, &func.return_type)?
        } else {
            Index::void()
        };

        let ret = self.nasm.get_local_label_name("call_return");

        write_asm!(self.nasm, "lea rax, [{ret}] ; codegen_call (ret)")?;
        self.nasm.push_register("rax", RegisterSize::S64)?;
        for ((val_ty, idx), fn_ty) in params.into_iter().zip(&func.params) {
            if *fn_ty != val_ty {
                return Err(Error::TypeMismatch(fn_ty.clone(), val_ty));
            }
            self.nasm.push_copy(&idx)?;
        }

        write_asm!(
            self.nasm,
            "jmp {} ; codegen_call (jmp)",
            self.nasm.get_global_label_name(&name)
        )?;
        self.nasm.raw_label(&ret)?;
        for _ in &func.params {
            self.nasm.pop_supress();
        }
        self.nasm.pop_supress(); // the called function will pop rax

        Ok((func.return_type.clone(), ret_idx))
    }

    pub fn codegen_call_c(
        &mut self,
        func: &CFunction,
        name: String,
        params: Vec<(ExpressionType, Index)>,
    ) -> Result<(ExpressionType, Index)> {
        if !func.is_variadic && func.params.len() != params.len() {
            return Err(Error::ArityMismatch(func.params.len(), params.len()));
        }
        if func.is_variadic && func.params.len() > params.len() {
            return Err(Error::ArityMismatch(func.params.len(), params.len()));
        }

        let p = params
            .into_iter()
            .enumerate()
            .map(|(x, (y, z))| (x, y, z))
            .rev()
            .collect::<Vec<(usize, ExpressionType, Index)>>();

        let reg64 = ["rdi", "rsi", "rdx", "rcx", "r8", "r9"];
        let reg32 = ["edi", "esi", "edx", "ecx", "r8d", "r9d"];
        let reg16 = ["di", "si", "dx", "cx", "r8w", "r9w"];
        let reg8 = ["dil", "sil", "dl", "cl", "r8b", "r9b"];
        let mut stack_size = 0;

        for (i, ty, idx) in p {
            let ExpressionType::Number(numty) = ty else {
                return Err(Error::ExternCInvalidParameter);
            };

            if let Some(reg) = reg64.get(i) {
                write_asm!(self.nasm, "xor {reg}, {reg}")?;
            }

            let (reg, size) = match numty {
                NumberType::I8 => (reg8.get(i), RegisterSize::S8),
                NumberType::I16 => (reg16.get(i), RegisterSize::S16),
                NumberType::I32 => (reg32.get(i), RegisterSize::S32),
                NumberType::I64 => (reg64.get(i), RegisterSize::S64),
                NumberType::U8 => (reg8.get(i), RegisterSize::S8),
                NumberType::U16 => (reg16.get(i), RegisterSize::S16),
                NumberType::U32 => (reg32.get(i), RegisterSize::S32),
                NumberType::U64 => (reg64.get(i), RegisterSize::S64),
                NumberType::F32 | NumberType::F64 => {
                    return Err(Error::ExternCInvalidParameter);
                }
            };

            match reg {
                Some(reg) => {
                    self.nasm.idx2addr("r10", &idx)?;
                    write_asm!(self.nasm, "mov {reg}, [r10+8]")?;
                }
                None => {
                    self.nasm.idx2addr("rbx", &idx)?;
                    write_asm!(self.nasm, "mov {}, [rbx+8]", size.b())?;
                    write_asm!(self.nasm, "push {}", size.b())?;
                    stack_size += size.size_bytes();
                }
            }
        }

        write_asm!(self.nasm, "xor eax, eax")?;
        let label = self.nasm.get_global_label_name(&name);
        write_asm!(self.nasm, "call {label}")?;

        if stack_size != 0 {
            write_asm!(self.nasm, "sub rsp, {stack_size}")?;
        }

        match &func.return_type {
            Some(return_type) => {
                let idx = self.nasm.push_register("rax", RegisterSize::S64)?;
                Ok((ExpressionType::Number(*return_type), idx))
            }
            None => Ok((ExpressionType::Void, Index::void())),
        }
    }

    pub fn codegen_binop(
        &mut self,
        (lhs, op, rhs): (Expression, String, Expression),
    ) -> Result<(ExpressionType, Index)> {
        let (lhs_type, lhs) = self.codegen_expression(lhs)?;
        let (rhs_type, rhs) = self.codegen_expression(rhs)?;
        let (lhs_type, rhs_type) = (lhs_type.into_number()?, rhs_type.into_number()?);
        let mut ret_type = None;

        let ret = match op.as_str() {
            "+" => self
                .nasm
                .sum("add", "addss", "addsd", lhs_type, rhs_type, &lhs, &rhs),
            "-" => self
                .nasm
                .sum("sub", "subss", "subsd", lhs_type, rhs_type, &lhs, &rhs),
            "*" => self
                .nasm
                .prod("mul", "imul", lhs_type, rhs_type, &lhs, &rhs, false),
            "/" => self
                .nasm
                .prod("div", "idiv", lhs_type, rhs_type, &lhs, &rhs, false),
            "%" => self
                .nasm
                .prod("div", "idiv", lhs_type, rhs_type, &lhs, &rhs, true),

            "&" => self.nasm.bitwise("and", lhs_type, rhs_type, &lhs, &rhs),
            "|" => self.nasm.bitwise("or", lhs_type, rhs_type, &lhs, &rhs),
            "^" => self.nasm.bitwise("xor", lhs_type, rhs_type, &lhs, &rhs),
            "<<" => self.nasm.bitshift("shl", lhs_type, rhs_type, &lhs, &rhs),
            ">>" => self.nasm.bitshift("shr", lhs_type, rhs_type, &lhs, &rhs),

            "&&" => {
                ret_type = Some(NumberType::U8);
                self.codegen_and_or((lhs_type, lhs), (rhs_type, rhs), "and")
            }
            "||" => {
                ret_type = Some(NumberType::U8);
                self.codegen_and_or((lhs_type, lhs), (rhs_type, rhs), "or")
            }

            ">" => {
                ret_type = Some(NumberType::U8);
                self.codegen_ord((lhs_type, lhs), (rhs_type, rhs), "g")
            }
            ">=" => {
                ret_type = Some(NumberType::U8);
                self.codegen_ord((lhs_type, lhs), (rhs_type, rhs), "ge")
            }
            "<" => {
                ret_type = Some(NumberType::U8);
                self.codegen_ord((lhs_type, lhs), (rhs_type, rhs), "l")
            }
            "<=" => {
                ret_type = Some(NumberType::U8);
                self.codegen_ord((lhs_type, lhs), (rhs_type, rhs), "le")
            }
            "==" => {
                ret_type = Some(NumberType::U8);
                self.codegen_ord((lhs_type, lhs), (rhs_type, rhs), "e")
            }
            "!=" => {
                ret_type = Some(NumberType::U8);
                self.codegen_ord((lhs_type, lhs), (rhs_type, rhs), "ne")
            }

            _ => return Err(Error::InvalidOperator(op)),
        }?;

        Ok((ExpressionType::Number(ret_type.unwrap_or(lhs_type)), ret))
    }

    pub fn codegen_ord(
        &mut self,
        (lhs_type, lhs): (NumberType, Index),
        (rhs_type, rhs): (NumberType, Index),
        operation: &str,
    ) -> Result<Index> {
        self.mov_num((lhs_type, lhs), "bl", "bx", "ebx", "rbx", "rdx")?;
        self.mov_num((rhs_type, rhs), "cl", "cx", "ecx", "rcx", "rdx")?;
        write_asm!(self.nasm, "cmp rbx, rcx")?;
        write_asm!(self.nasm, "set{operation} bl")?;
        self.nasm.push_register("bl", RegisterSize::S8)
    }

    pub fn codegen_and_or(
        &mut self,
        (lhs_type, lhs): (NumberType, Index),
        (rhs_type, rhs): (NumberType, Index),
        operation: &str,
    ) -> Result<Index> {
        self.mov_num((lhs_type, lhs), "bl", "bx", "ebx", "rbx", "rcx")?;
        write_asm!(self.nasm, "cmp rbx, 0")?;
        write_asm!(self.nasm, "setne bl")?;

        self.mov_num((rhs_type, rhs), "cl", "cx", "ecx", "rcx", "rdx")?;
        write_asm!(self.nasm, "cmp rcx, 0")?;
        write_asm!(self.nasm, "setne cl")?;

        write_asm!(self.nasm, "{operation} bl, cl")?;

        self.nasm.push_register("bl", RegisterSize::S8)
    }

    pub fn codegen_number(&mut self, number: Number) -> Result<(ExpressionType, Index)> {
        let idx = self.nasm.push(number.size_bytes(), "number")?;
        let numtype = number.numtype();

        match number {
            Number::I8(value) => {
                write_asm!(self.nasm, "mov byte [rsp+8], {value} ; push_number (i8)")
            }
            Number::I16(value) => {
                write_asm!(self.nasm, "mov word [rsp+8], {value} ; push_number (i16)")
            }
            Number::I32(value) => {
                write_asm!(self.nasm, "mov dword [rsp+8], {value} ; push_number (i32)")
            }
            Number::I64(value) => {
                write_asm!(self.nasm, "mov qword [rsp+8], {value} ; push_number (i64)")
            }
            Number::U8(value) => {
                write_asm!(self.nasm, "mov byte [rsp+8], {value} ; push_number (u8)")
            }
            Number::U16(value) => {
                write_asm!(self.nasm, "mov word [rsp+8], {value} ; push_number (u16)")
            }
            Number::U32(value) => {
                write_asm!(self.nasm, "mov dword [rsp+8], {value} ; push_number (u32)")
            }
            Number::U64(value) => {
                write_asm!(self.nasm, "mov qword [rsp+8], {value} ; push_number (u64)")
            }
            Number::F32(value) => write_asm!(
                self.nasm,
                "mov dword [rsp+8], {} ; push_number (f32)",
                value.to_bits()
            ),
            Number::F64(value) => write_asm!(
                self.nasm,
                "mov qword [rsp+8], {} ; push_number (f64)",
                value.to_bits()
            ),
        }?;

        Ok((ExpressionType::Number(numtype), idx))
    }

    /// move a number into a register
    pub fn mov_num(
        &mut self,
        (cond_type, cond_idx): (NumberType, Index),
        reg8: &str,
        reg16: &str,
        reg32: &str,
        reg64: &str,
        sreg64: &str,
    ) -> Result<()> {
        write_asm!(self.nasm, "xor {reg64}, {reg64}")?;
        self.nasm.idx2addr(sreg64, &cond_idx)?;

        match cond_type {
            NumberType::I8 => write_asm!(self.nasm, "mov {reg8}, byte [{sreg64}+8]")?,
            NumberType::I16 => write_asm!(self.nasm, "mov {reg16}, word [{sreg64}+8]")?,
            NumberType::I32 => write_asm!(self.nasm, "mov {reg32}, dword [{sreg64}+8]")?,
            NumberType::I64 => write_asm!(self.nasm, "mov {reg64}, qword [{sreg64}+8]")?,
            NumberType::U8 => write_asm!(self.nasm, "mov {reg8}, byte [{sreg64}+8]")?,
            NumberType::U16 => write_asm!(self.nasm, "mov {reg16}, word [{sreg64}+8]")?,
            NumberType::U32 => write_asm!(self.nasm, "mov {reg32}, dword [{sreg64}+8]")?,
            NumberType::U64 => write_asm!(self.nasm, "mov {reg64}, qword [{sreg64}+8]")?,
            ty @ NumberType::F32 | ty @ NumberType::F64 => {
                return Err(Error::ExpectedInteger(ExpressionType::Number(ty)));
            }
        }

        Ok(())
    }
}

#[derive(Default, Clone)]
pub struct VarTable {
    outer: Option<Box<VarTable>>,
    data: HashMap<String, (ExpressionType, Index)>,
}

impl VarTable {
    pub fn new(outer: Option<VarTable>) -> Self {
        Self {
            outer: outer.map(Box::new),
            data: HashMap::new(),
        }
    }

    pub fn get_symbol(&self, k: String) -> Result<(ExpressionType, Index)> {
        match self.data.get(&k) {
            Some(x) => Ok(x.clone()),
            None => match &self.outer {
                Some(outer) => outer.get_symbol(k),
                None => Err(Error::UnknownSymbol(k)),
            },
        }
    }
}

pub struct Nasm<W: io::Write> {
    writer: W,
    uniq_index: usize,
    stack_pointer: u16,
    rodata: Cursor<Vec<u8>>,
}

impl<W: io::Write> Nasm<W> {
    pub fn new(writer: W) -> Self {
        Self {
            writer,
            uniq_index: 0,
            stack_pointer: 0,
            rodata: Cursor::new(Vec::new()),
        }
    }

    pub fn finalize(&mut self) -> Result<()> {
        writeln!(self.writer)?;
        writeln!(self.writer, "section .data")?;
        self.rodata.rewind()?;
        io::copy(&mut self.rodata, &mut self.writer)?;
        Ok(())
    }
}

impl<W: io::Write> Nasm<W> {
    pub fn db_cstr(&mut self, msg: &str, reg: &str) -> Result<String> {
        let mut ret = String::from('"');
        for ch in msg.as_bytes() {
            if *ch >= 32 && *ch <= 126 {
                ret.push(*ch as char);
            } else {
                ret.push_str(&format!("\", {ch}, \""));
            }
        }
        ret.push_str("\", 0");
        let msg = ret.replace(", \"\"", "");

        let uniq_name = format!("db_string_{}", self.uniq_index);
        self.uniq_index += 1;

        write_rdat!(self, "{uniq_name}: db {msg}")?;
        write_asm!(self, "mov {reg}, {uniq_name}")?;

        Ok(uniq_name)
    }
}

impl<W: io::Write> Nasm<W> {
    pub fn sum(
        &mut self,
        op: &'static str,
        opss: &'static str,
        opsd: &'static str,
        lhs_type: NumberType,
        rhs_type: NumberType,
        lhs: &Index,
        rhs: &Index,
    ) -> Result<Index> {
        if lhs_type != rhs_type {
            return Err(Error::TypeMismatch(
                ExpressionType::Number(lhs_type),
                ExpressionType::Number(rhs_type),
            ));
        }

        match lhs_type {
            NumberType::I8 => self.op(op, RegisterSize::S8, lhs, rhs),
            NumberType::I16 => self.op(op, RegisterSize::S16, lhs, rhs),
            NumberType::I32 => self.op(op, RegisterSize::S32, lhs, rhs),
            NumberType::I64 => self.op(op, RegisterSize::S64, lhs, rhs),
            NumberType::U8 => self.op(op, RegisterSize::S8, lhs, rhs),
            NumberType::U16 => self.op(op, RegisterSize::S16, lhs, rhs),
            NumberType::U32 => self.op(op, RegisterSize::S32, lhs, rhs),
            NumberType::U64 => self.op(op, RegisterSize::S64, lhs, rhs),
            NumberType::F32 => self.op(opss, RegisterSize::S32, lhs, rhs),
            NumberType::F64 => self.op(opsd, RegisterSize::S64, lhs, rhs),
        }
    }

    pub fn prod(
        &mut self,
        op: &'static str,
        iop: &'static str,
        lhs_type: NumberType,
        rhs_type: NumberType,
        lhs: &Index,
        rhs: &Index,
        use_regd: bool,
    ) -> Result<Index> {
        if lhs_type != rhs_type {
            return Err(Error::TypeMismatch(
                ExpressionType::Number(lhs_type),
                ExpressionType::Number(rhs_type),
            ));
        }

        match lhs_type {
            NumberType::I8 => self.op_implied_reg(iop, RegisterSize::S8, lhs, rhs, use_regd),
            NumberType::I16 => self.op_implied_reg(iop, RegisterSize::S16, lhs, rhs, use_regd),
            NumberType::I32 => self.op_implied_reg(iop, RegisterSize::S32, lhs, rhs, use_regd),
            NumberType::I64 => self.op_implied_reg(iop, RegisterSize::S64, lhs, rhs, use_regd),
            NumberType::U8 => self.op_implied_reg(op, RegisterSize::S8, lhs, rhs, use_regd),
            NumberType::U16 => self.op_implied_reg(op, RegisterSize::S16, lhs, rhs, use_regd),
            NumberType::U32 => self.op_implied_reg(op, RegisterSize::S32, lhs, rhs, use_regd),
            NumberType::U64 => self.op_implied_reg(op, RegisterSize::S64, lhs, rhs, use_regd),
            NumberType::F32 => todo!("float math"),
            NumberType::F64 => todo!("float math"),
        }
    }

    pub fn bitwise(
        &mut self,
        op: &'static str,
        lhs_type: NumberType,
        rhs_type: NumberType,
        lhs: &Index,
        rhs: &Index,
    ) -> Result<Index> {
        if lhs_type != rhs_type {
            return Err(Error::TypeMismatch(
                ExpressionType::Number(lhs_type),
                ExpressionType::Number(rhs_type),
            ));
        }

        match lhs_type {
            NumberType::I8 => self.op(op, RegisterSize::S8, lhs, rhs),
            NumberType::I16 => self.op(op, RegisterSize::S16, lhs, rhs),
            NumberType::I32 => self.op(op, RegisterSize::S32, lhs, rhs),
            NumberType::I64 => self.op(op, RegisterSize::S64, lhs, rhs),
            NumberType::U8 => self.op(op, RegisterSize::S8, lhs, rhs),
            NumberType::U16 => self.op(op, RegisterSize::S16, lhs, rhs),
            NumberType::U32 => self.op(op, RegisterSize::S32, lhs, rhs),
            NumberType::U64 => self.op(op, RegisterSize::S64, lhs, rhs),
            NumberType::F32 | NumberType::F64 => Err(Error::BitwiseFloat),
        }
    }

    pub fn bitshift(
        &mut self,
        op: &'static str,
        lhs_type: NumberType,
        rhs_type: NumberType,
        lhs: &Index,
        rhs: &Index,
    ) -> Result<Index> {
        if rhs_type != NumberType::U16 {
            return Err(Error::BitShiftU16);
        }

        match lhs_type {
            NumberType::I8 => self.op_bitshift(op, RegisterSize::S8, lhs, rhs),
            NumberType::I16 => self.op_bitshift(op, RegisterSize::S16, lhs, rhs),
            NumberType::I32 => self.op_bitshift(op, RegisterSize::S32, lhs, rhs),
            NumberType::I64 => self.op_bitshift(op, RegisterSize::S64, lhs, rhs),
            NumberType::U8 => self.op_bitshift(op, RegisterSize::S8, lhs, rhs),
            NumberType::U16 => self.op_bitshift(op, RegisterSize::S16, lhs, rhs),
            NumberType::U32 => self.op_bitshift(op, RegisterSize::S32, lhs, rhs),
            NumberType::U64 => self.op_bitshift(op, RegisterSize::S64, lhs, rhs),
            NumberType::F32 | NumberType::F64 => Err(Error::BitwiseFloat),
        }
    }

    pub fn op_implied_reg(
        &mut self,
        operation: &'static str,
        size: RegisterSize,
        lhs: &Index,
        rhs: &Index,
        use_regd: bool,
    ) -> Result<Index> {
        assert!(operation.chars().all(|c| char::is_ascii_alphabetic(&c)));

        let rega = size.a();
        let regc = size.c();
        let regd = size.d();

        self.idx2addr("rbx", rhs)?;
        write_asm!(self, "mov {regc}, [rbx+8] ; {operation}_{size:?}")?;
        self.idx2addr("rbx", lhs)?;
        write_asm!(self, "mov {rega}, [rbx+8] ; {operation}_{size:?}")?;

        write_asm!(self, "{operation} {regc} ; {operation}_{size:?}")?;
        let idx = self.push_register(if use_regd { regd } else { rega }, size)?;

        Ok(idx)
    }

    pub fn op_bitshift(
        &mut self,
        operation: &'static str,
        size: RegisterSize,
        lhs: &Index,
        rhs: &Index,
    ) -> Result<Index> {
        assert!(operation.chars().all(|c| char::is_ascii_alphabetic(&c)));

        let regb = size.b();

        self.idx2addr("rbx", rhs)?;
        write_asm!(self, "mov cl, [rbx+8] ; {operation}_{size:?}")?;
        self.idx2addr("rbx", lhs)?;
        write_asm!(self, "mov {regb}, [rbx+8] ; {operation}_{size:?}")?;

        write_asm!(self, "{operation} {regb}, cl ; {operation}_{size:?}")?;
        let idx = self.push_register(regb, size)?;

        Ok(idx)
    }

    pub fn op(
        &mut self,
        operation: &'static str,
        size: RegisterSize,
        lhs: &Index,
        rhs: &Index,
    ) -> Result<Index> {
        assert!(operation.chars().all(|c| char::is_ascii_alphabetic(&c)));

        let regc = size.c();
        let regb = size.b();

        self.idx2addr("rbx", rhs)?;
        write_asm!(self, "mov {regc}, [rbx+8] ; {operation}_{size:?}")?;
        self.idx2addr("rbx", lhs)?;
        write_asm!(self, "mov {regb}, [rbx+8] ; {operation}_{size:?}")?;

        write_asm!(self, "{operation} {regb}, {regc} ; {operation}_{size:?}")?;
        let idx = self.push_register(regb, size)?;

        Ok(idx)
    }
}

#[derive(Debug)]
pub enum RegisterSize {
    S64,
    S32,
    S16,
    S8,
}

impl RegisterSize {
    pub fn a(&self) -> &'static str {
        match self {
            RegisterSize::S64 => "rax",
            RegisterSize::S32 => "eax",
            RegisterSize::S16 => "ax",
            RegisterSize::S8 => "al",
        }
    }

    pub fn b(&self) -> &'static str {
        match self {
            RegisterSize::S64 => "rbx",
            RegisterSize::S32 => "ebx",
            RegisterSize::S16 => "bx",
            RegisterSize::S8 => "bl",
        }
    }

    pub fn c(&self) -> &'static str {
        match self {
            RegisterSize::S64 => "rcx",
            RegisterSize::S32 => "ecx",
            RegisterSize::S16 => "cx",
            RegisterSize::S8 => "cl",
        }
    }

    pub fn d(&self) -> &'static str {
        match self {
            RegisterSize::S64 => "rdx",
            RegisterSize::S32 => "edx",
            RegisterSize::S16 => "dx",
            RegisterSize::S8 => "dl",
        }
    }

    pub fn size_bytes(&self) -> u16 {
        match self {
            RegisterSize::S64 => 64 / 8,
            RegisterSize::S32 => 32 / 8,
            RegisterSize::S16 => 16 / 8,
            RegisterSize::S8 => 8 / 8,
        }
    }
}

impl<W: io::Write> Nasm<W> {
    /// Push a register onto the stack.
    pub fn push_register(&mut self, reg: &str, size: RegisterSize) -> Result<Index> {
        assert!(reg.chars().all(|c| char::is_ascii_alphabetic(&c)));

        let idx = self.push(size.size_bytes(), "reg")?;
        write_asm!(self, "mov [rsp+8], {reg} ; push_register")?;

        Ok(idx)
    }

    /// Pop a register from the stack. Setting the register to the value stored inside it.
    pub fn pop_register(&mut self, reg: &str) -> Result<()> {
        assert!(reg.chars().all(|c| char::is_ascii_alphabetic(&c)));

        write_asm!(self, "mov {reg}, [rsp+8] ; pop_register")?;
        self.pop()?;

        Ok(())
    }

    /// Push data onto the stack and returns the index of that data.
    /// This will allocate an extra 8-bytes to store the size of the data.
    /// If you want to access the data you'll have to do `rsp+8`
    pub fn push(&mut self, size: u16, name: &str) -> Result<Index> {
        let idx = self.stack_pointer;
        self.inc_stack()?;

        let size = size + 8;
        write_asm!(self, "sub rsp, {size} ; push ({name})")?;
        write_asm!(self, "mov qword [rsp], {size} ; push ({name})")?;

        Ok(Index::new(idx))
    }

    /// Manually push a size tag stored in a register onto the stack.
    /// The `size` parameter is the size of the data including the 8-byte size tag.
    /// `push_size_tag(8)` will store data with a size of `0` because the size tag takes 8-bytes.
    pub fn push_stag_reg(&mut self, reg: &str) -> Result<Index> {
        let idx = self.stack_pointer;
        self.inc_stack()?;

        write_asm!(self, "sub rsp, 8 ; push_size_tag")?;
        write_asm!(self, "mov qword [rsp], {reg} ; push_size_tag")?;

        Ok(Index::new(idx))
    }

    /// Manually push a size tag stored in a register onto the stack.
    /// The `size` parameter is the size of the data including the 8-byte size tag.
    /// `push_size_tag(8)` will store data with a size of `0` because the size tag takes 8-bytes.
    pub fn push_stag_val(&mut self, size: u16) -> Result<Index> {
        let size = size + 8;
        let idx = self.stack_pointer;
        self.inc_stack()?;

        write_asm!(self, "sub rsp, 8 ; push_size_tag")?;
        write_asm!(self, "mov qword [rsp], {size} ; push_size_tag")?;

        Ok(Index::new(idx))
    }

    /// Manually push a size tag stored in a register onto the stack.
    /// The `size` parameter is the size of the data including the 8-byte size tag.
    /// `push_size_tag(8)` will store data with a size of `0` because the size tag takes 8-bytes.
    pub fn push_stag_hidden(&mut self, size: u16) -> Result<u16> {
        let size = size + 8;
        write_asm!(self, "sub rsp, 8 ; push_size_tag")?;
        write_asm!(self, "mov qword [rsp], {size} ; push_size_tag")?;
        Ok(size)
    }

    /// Push data onto the stack but don't increment the stack pointer, return the size allocated.
    /// This is used to push data into an array because the data isn't on the stack but inside the array.
    pub fn push_hidden(&mut self, size: u16) -> Result<u16> {
        let size = size + 8;
        write_asm!(self, "sub rsp, {size} ; push_hidden")?;
        write_asm!(self, "mov qword [rsp], {size} ; push_hidden")?;
        Ok(size)
    }

    /// Push data onto the stack but don't generate any assembly instructions to do so.
    /// Use this when external code pushes from the stack.
    pub fn push_supress(&mut self) -> Index {
        let idx = self.stack_pointer;
        self.stack_pointer += 1;
        Index::new(idx)
    }

    /// Push a copy of some data onto the top of the stack.
    pub fn push_copy(&mut self, idx: &Index) -> Result<Index> {
        self.idx2addr("rsi", idx)?; // rsi = src ptr
        self.push_copy_addr()
    }

    /// Push a copy of the data `rsi` points to onto the top of the stack.
    pub fn push_copy_addr(&mut self) -> Result<Index> {
        let idx = self.stack_pointer;
        self.inc_stack()?;
        write_asm!(self, "mov rbx, [rsi] ; push_copy (rbx=size)")?; // rbx = size

        write_asm!(self, "sub rsp, rbx ; push_copy (rsp-=size)")?; // rdi = dest ptr (src ptr - size)
        write_asm!(self, "mov rdi, rsp ; push_copy (rdi=dest)")?; // rdi = dest ptr (src ptr - size)

        // for (; rbx > 0; rbx--)
        let ret = self.get_local_label_name("push_return");

        let body = self.local_label("push_body")?;
        write_asm!(self, "cmp rbx, 0")?;
        write_asm!(self, "je {ret}")?;

        // *rdi = *rsi
        write_asm!(self, "mov cl, [rsi] ; push_copy (*rdi = *rsi)")?;
        write_asm!(self, "mov [rdi], cl")?;

        // rdi, rsi += 1, 1
        write_asm!(self, "add rsi, 1 ; push_copy (rdi,rsi+=1)")?;
        write_asm!(self, "add rdi, 1")?;

        write_asm!(self, "sub rbx, 1 ; push_copy (size-=1)")?;
        write_asm!(self, "jmp {body}")?;

        self.raw_label(&ret)?;

        Ok(Index::new(idx))
    }

    /// Copy data from the address `rsi` to `rdi`.
    /// Unlike `copy_raw` this function will resize `rdi` to fit `rsi`.
    pub fn copy_addr(&mut self) -> Result<()> {
        let ret = self.get_local_label_name("copy_return");

        write_asm!(self, "lea rax, [{ret}]")?;
        write_asm!(self, "jmp copy_addr")?;

        self.raw_label(&ret)?;
        Ok(())
    }

    pub fn copy_addr_impl(&mut self) -> Result<()> {
        self.push_register("rsi", RegisterSize::S64)?;
        self.push_register("rdi", RegisterSize::S64)?;

        write_asm!(self, "mov rbx, rdi")?;
        write_asm!(self, "mov rdx, [rsi]")?;
        self.resize_impl()?;

        self.pop_register("rdi")?;
        self.pop_register("rsi")?;

        // since resizing pushed the pointers around
        // we have to re-adjust our pointers.
        // rbx is returned by `resize_impl`
        write_asm!(self, "sub rbx, rdi")?;
        write_asm!(self, "add rsi, rbx")?;
        write_asm!(self, "add rdi, rbx")?;

        self.copy_raw_impl()?;

        Ok(())
    }

    /// Copy data from `rsi` to `rdi`.
    /// Expects that the data at `rsi` and `rdi` will be the same size.
    pub fn copy_raw_impl(&mut self) -> Result<()> {
        write_asm!(self, "mov rbx, [rsi] ; copy (size=*rsi)")?;

        let ret = self.get_local_label_name("copy_return");

        let body = self.local_label("copy_body")?;
        write_asm!(self, "cmp rbx, 0")?;
        write_asm!(self, "je {ret}")?;

        write_asm!(self, "mov cl, [rsi]")?;
        write_asm!(self, "mov [rdi], cl")?;

        write_asm!(self, "add rsi, 1")?;
        write_asm!(self, "add rdi, 1")?;

        write_asm!(self, "sub rbx, 1")?;
        write_asm!(self, "jmp {body}")?;

        self.raw_label(&ret)?;

        Ok(())
    }

    /// Set the size of the data at the address in `rbx` to the value specified in `rdx`.
    /// Stores the address of the new data in `rbx`.
    ///
    /// Note: the new size in `rdx` includes the 8-byte size tag that is at the start of every stack allocation.
    pub fn resize_impl(&mut self) -> Result<()> {
        let ret = self.get_local_label_name("resize_return");
        let negative = self.get_local_label_name("resize_negative");

        write_asm!(self, "sub rdx, [rbx] ; resize")?;
        write_asm!(self, "js {negative} ; resize")?;

        self.grow()?;
        write_asm!(self, "jmp {ret} ; resize")?;

        self.raw_label(&negative)?;
        write_asm!(self, "neg rdx ; resize")?; // abs of rdx
        self.shrink()?;

        self.raw_label(&ret)?;

        Ok(())
    }

    /// Shrink the size of the data at the address in `rbx` by an amount specified in `rdx`.
    /// Stores the address of the new data in `rbx`.
    pub fn shrink(&mut self) -> Result<()> {
        // set rcx = old size
        write_asm!(self, "mov rcx, [rbx] ; shrink")?;
        // set rsi = old src
        write_asm!(self, "mov rsi, rbx ; shrink")?;

        // set rbx += size
        write_asm!(self, "add rbx, rdx ; shrink")?;
        // set *rbx = old size - amount
        write_asm!(self, "sub rcx, rdx ; shrink")?;
        write_asm!(self, "mov [rbx], rcx ; shrink")?;
        // set rdi = new src
        write_asm!(self, "mov rdi, rbx ; shrink")?;

        let ret = self.get_local_label_name("shrink_return");

        let body = self.local_label("shrink_body")?;

        write_asm!(self, "cmp rsi, rsp ; shrink")?;
        write_asm!(self, "je {ret} ; shrink")?;

        write_asm!(self, "sub rsi, 1 ; shrink")?;
        write_asm!(self, "sub rdi, 1 ; shrink")?;

        write_asm!(self, "mov cl, [rsi] ; shrink")?;
        write_asm!(self, "mov [rdi], cl ; shrink")?;

        write_asm!(self, "jmp {body} ; shrink")?;

        self.raw_label(&ret)?;
        write_asm!(self, "add rsp, rdx ; shrink")?;

        Ok(())
    }

    /// Grow the size of the data at the address in `rbx` by an amount specified in `rdx`.
    /// Stores the address of the new data in `rbx`.
    pub fn grow(&mut self) -> Result<()> {
        write_asm!(self, "mov rsi, rsp ; grow")?;
        write_asm!(self, "sub rsp, rdx ; grow")?;
        write_asm!(self, "mov rdi, rsp ; grow")?;

        let ret = self.get_local_label_name("grow_return");

        let body = self.local_label("grow_body")?;
        write_asm!(self, "cmp rsi, rbx ; grow")?;
        write_asm!(self, "je {ret} ; grow")?;

        write_asm!(self, "mov cl, [rsi] ; grow")?;
        write_asm!(self, "mov [rdi], cl ; grow")?;

        write_asm!(self, "add rsi, 1 ; grow")?;
        write_asm!(self, "add rdi, 1 ; grow")?;
        write_asm!(self, "jmp {body} ; grow")?;

        self.raw_label(&ret)?;

        write_asm!(self, "mov rcx, [rbx] ; grow (set size)")?;
        write_asm!(self, "add rcx, rdx ; grow")?;
        write_asm!(self, "sub rbx, rdx ; grow")?;
        write_asm!(self, "mov [rbx], rcx ; grow")?;

        Ok(())
    }

    /// Converts an index into an address. Stores the address in `ret_reg`.
    pub fn idx2addr(&mut self, ret_reg: &str, idx: &Index) -> Result<()> {
        let idx = idx.valid()?;
        let steps = self.stack_pointer - idx.0 - 1;
        write_asm!(self, "mov {ret_reg}, rsp ; idx2addr ({idx:?})")?;
        self.idx2addr_jump_back(ret_reg, steps)?;
        for idx in &idx.1 {
            write_asm!(self, "add {ret_reg}, 8 ; idx2addr")?;
            self.idx2addr_jump_back(ret_reg, *idx)?;
        }
        Ok(())
    }

    fn idx2addr_jump_back(&mut self, ret_reg: &str, steps: u16) -> Result<()> {
        for i in 0..steps {
            write_asm!(
                self,
                "add {ret_reg}, [{ret_reg}] ; idx2addr ({})",
                steps - i - 1
            )?;
        }

        Ok(())
    }

    /// Given the reference at the address stored in `rbx`. Find the address and store it in `rsi`.
    pub fn ref2addr(&mut self) -> Result<()> {
        let ret = self.get_local_label_name("ref2addr_return");

        write_asm!(self, "lea rax, [{ret}]")?;
        write_asm!(self, "jmp ref2addr")?;

        self.raw_label(&ret)?;
        Ok(())
    }

    pub fn ref2addr_impl(&mut self) -> Result<()> {
        // rcx = stop ptr
        write_asm!(self, "mov rcx, rbx")?;
        write_asm!(self, "add rcx, [rcx]")?;
        // skip size tag
        write_asm!(self, "add rbx, 8")?;

        // jump back by (rbp - fst)
        write_asm!(self, "mov dx, bp")?;
        write_asm!(self, "sub dx, [rbx]")?;
        write_asm!(self, "mov rsi, rsp")?;
        self.ref2addr_jump_back("rsi", "dx")?;

        let ret = self.get_local_label_name("ref2addr_return");

        let body = self.local_label("ref2addr_body")?;
        write_asm!(self, "add rbx, 2")?;
        write_asm!(self, "cmp rbx, rcx")?;
        write_asm!(self, "je {ret}")?;
        write_asm!(self, "add rsi, 8")?;
        write_asm!(self, "mov dx, [rbx]")?;
        self.ref2addr_jump_back("rsi", "dx")?;
        write_asm!(self, "jmp {body}")?;
        self.raw_label(&ret)?;

        Ok(())
    }

    /// From `ptr` jump back in the stack `count` amount of times.
    fn ref2addr_jump_back(&mut self, ptr: &str, count: &str) -> Result<()> {
        let ret = self.get_local_label_name("ref2addr_jump_back_return");

        let body = self.local_label("ref2addr_jump_back_body")?;
        write_asm!(self, "cmp {count}, 0 ; ref2addr_jump_back")?;
        write_asm!(self, "je {ret} ; ref2addr_jump_back")?;

        write_asm!(self, "add {ptr}, [{ptr}] ; ref2addr_jump_back")?;

        write_asm!(self, "sub {count}, 1 ; ref2addr_jump_back")?;
        write_asm!(self, "jmp {body} ; ref2addr_jump_back")?;

        self.raw_label(&ret)?;

        Ok(())
    }

    /// Converts an index into a reference. Stores the reference on the stack.
    pub fn push_idx2ref(&mut self, idx: &Index, s: &str) -> Result<Index> {
        let idx = idx.valid()?;
        let ref_idx = self.push((2 + idx.1.len() * 2) as u16, "ref")?;

        let steps = self.stack_pointer - idx.0 - 1;
        write_asm!(self, "mov {s}, bp ; idx2ref ({idx:?})")?;
        write_asm!(self, "sub {s}, {steps} ; idx2ref")?;
        write_asm!(self, "mov word [rsp+8], {s}")?;

        for (i, &x) in idx.1.iter().enumerate() {
            let off = 8 + 2 + i * 2;
            write_asm!(self, "mov word [rsp+{off}], {x}")?;
        }

        Ok(ref_idx)
    }

    /// Push data from the top of the stack.
    /// Guaranteed not to clobber any registers.
    pub fn pop(&mut self) -> Result<()> {
        assert!(self.stack_pointer > 0);
        self.dec_stack()?;
        write_asm!(self, "add rsp, [rsp] ; pop")?;
        Ok(())
    }

    /// Keep popping until the stack pointer is restored to a certain point.
    pub fn pop_until(&mut self, ptr: u16) -> Result<()> {
        for _ in ptr..self.stack_pointer {
            self.pop()?;
        }
        Ok(())
    }

    /// Return from the function.
    pub fn ret(&mut self, return_type: &ExpressionType) -> Result<()> {
        // If the return type is not void, the first two items in the stack are the return value and the return address.
        // If the return type IS void, the first item is the return address.
        // these need to be popped off in a special way.
        let start = if *return_type != ExpressionType::Void {
            2
        } else {
            1
        };

        let end = self.stack_pointer;

        write_asm!(self, "; return")?;
        for _ in start..self.stack_pointer {
            write_asm!(self, "add rsp, [rsp] ; return (pop)")?;
        }
        write_asm!(self, "sub rbp, {} ; return (dec_stack)", end - start)?;
        self.pop_register("rax")?;
        write_asm!(self, "jmp rax")?;
        Ok(())
    }

    fn dec_stack(&mut self) -> Result<()> {
        self.stack_pointer -= 1;
        write_asm!(self, "sub rbp, 1 ; dec_stack")?;
        Ok(())
    }

    fn inc_stack(&mut self) -> Result<()> {
        self.stack_pointer += 1;
        write_asm!(self, "add rbp, 1 ; inc_stack")?;
        Ok(())
    }

    /// Pop data from the top of the stack but don't generate any assembly instructions to do so.
    /// Use this when external code pops from the stack.
    pub fn pop_supress(&mut self) {
        assert!(self.stack_pointer > 0);
        self.stack_pointer -= 1;
    }

    /// Reset the stack pointer. Use this when you are writing a new function with a different call stack.
    /// Doesn't generate any pop instructions.
    pub fn new_stack(&mut self) -> Result<()> {
        self.stack_pointer = 0;
        Ok(())
    }
}

impl<W: io::Write> Nasm<W> {
    /// Creates a global label and returns it's escaped name.
    pub fn global_label(&mut self, name: &str) -> Result<String> {
        writeln!(self.writer)?;
        let name = format!("_{name}");
        self.raw_label(&name)?;
        Ok(name)
    }

    /// Get the escaped name of a global label.
    pub fn get_global_label_name(&self, name: &str) -> String {
        format!("_{name}")
    }

    /// Creates a local label and returns it's escaped name.
    pub fn local_label(&mut self, name: &str) -> Result<String> {
        let name = self.get_local_label_name(name);
        self.raw_label(&name)?;
        Ok(name)
    }

    /// Get the escaped name of a local label.
    pub fn get_local_label_name(&self, name: &str) -> String {
        format!("._{name}_{}", self.uniq_index)
    }

    /// Write a label directly without any string escaping.
    pub fn raw_label(&mut self, name: &str) -> Result<()> {
        Self::validate_symbol(name);
        writeln!(self.writer, "{name}:")?;
        self.uniq_index += 1;
        Ok(())
    }

    fn validate_symbol(name: &str) {
        fn is_valid_first_char(ch: char) -> bool {
            ch.is_alphabetic() || "._?".contains(ch)
        }
        fn is_valid_char(ch: char) -> bool {
            ch.is_alphanumeric() || "_$#@.?".contains(ch)
        }

        assert!(name.chars().next().is_some_and(is_valid_first_char));
        assert!(name.chars().all(is_valid_char));
    }
}

#[derive(Clone)]
pub struct Index(Option<ValidIndex>);

impl fmt::Debug for Index {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Some(idx) => write!(f, "{idx:?}"),
            None => write!(f, "[void]"),
        }
    }
}

#[derive(Clone)]
pub struct ValidIndex(u16, Vec<u16>);

impl fmt::Debug for ValidIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<{}>", self.0)?;
        for x in &self.1 {
            write!(f, "[{x}]")?;
        }
        Ok(())
    }
}

impl Index {
    pub fn new(value: u16) -> Self {
        Self(Some(ValidIndex(value, vec![])))
    }

    pub fn void() -> Self {
        Self(None)
    }

    pub fn valid_mut(&mut self) -> Result<&mut ValidIndex> {
        match &mut self.0 {
            Some(idx) => Ok(idx),
            None => Err(Error::InvalidIndex),
        }
    }

    pub fn valid(&self) -> Result<&ValidIndex> {
        match &self.0 {
            Some(idx) => Ok(idx),
            None => Err(Error::InvalidIndex),
        }
    }
}

#[macro_export]
macro_rules! write_asm {
    ($dst:expr, $($arg:tt)*) => {{
        write!($dst.writer, "    ")
            .and_then(|_| writeln!($dst.writer, $($arg)*))
    }};
}

#[macro_export]
macro_rules! write_rdat {
    ($dst:expr, $($arg:tt)*) => {{
        use std::io::Write;
        write!($dst.rodata, "    ")
            .and_then(|_| writeln!($dst.rodata, $($arg)*))
    }};
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("type mismatch: {0} and {1}")]
    TypeMismatch(ExpressionType, ExpressionType),
    #[error("type mismatch: expected any number but got {0}")]
    ExpectedNumber(ExpressionType),
    #[error("type mismatch: expected any integer but got {0}")]
    ExpectedInteger(ExpressionType),
    #[error("type mismatch: expected any reference but got {0}")]
    ExpectedRef(ExpressionType),
    #[error("invalid operator: {0}")]
    InvalidOperator(String),
    #[error("unknown symbol: {0}")]
    UnknownSymbol(String),
    #[error("unknown function: {0}")]
    UnknownFunction(String),
    #[error("arity mismatch: expected {0} parameters but got {1}")]
    ArityMismatch(usize, usize),
    #[error("unknown type: {0}")]
    UnknownType(String),
    #[error("invalid signature for main function, expected 'i32 ()'")]
    MainInvalidSignature,
    #[error("struct.field access is only applicable to structs")]
    CannotAccessStruct,
    #[error("array[_] access is only applicable to array references, use `&arr`")]
    CannotAccessArray,
    #[error("unknown field {0}.{0}")]
    UnknownField(String, String),
    #[error("cannot allocate a void type")]
    CannotAllocVoid,
    #[error("type mismatch: void (invalid index)")]
    InvalidIndex,
    #[error("cannot use bitwise operators on floats")]
    BitwiseFloat,
    #[error("can only bit-shift by a u16")]
    BitShiftU16,
    #[error("cannot give a function body to extern functions")]
    ExternFunctionBody,
    #[error("only non-float numeric types can be used in extern \"C\" functions")]
    ExternCInvalidParameter,
    #[error("stucker functions cannot be variadic")]
    VariadicStuckerFunction,
}
