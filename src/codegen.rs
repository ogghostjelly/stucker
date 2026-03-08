use std::{
    collections::HashMap,
    io::{self, Cursor, Seek},
};

use crate::{
    ast::{
        DefAssignment, Expression, ExpressionType, Function, GlobalValue, Number, NumberType,
        SetAssignment, Statement,
    },
    write_asm,
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

pub struct DefinedFunction {
    return_type: ExpressionType,
    params: Vec<ExpressionType>,
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
        write_asm!(self.nasm, "global _start")?;
        write_asm!(self.nasm, "section .text")?;

        self.nasm.raw_label("_start")?;
        write_asm!(self.nasm, "mov rbp, 0")?;

        let (_, idx) = FunctionCodegen::new(&mut self.nasm, &mut VarTable::default(), &self.def)
            .codegen_call(
                &DefinedFunction {
                    return_type: ExpressionType::Number(NumberType::I32),
                    params: vec![],
                },
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
                } = function;

                if name == "main" {
                    if return_type != ExpressionType::Number(NumberType::I32) {
                        return Err(Error::MainInvalidSignature);
                    }
                    if !params.is_empty() {
                        return Err(Error::MainInvalidSignature);
                    }
                }

                self.codegen_prologue(&return_type, &name)?;

                let mut def_params = vec![];
                let mut var = VarTable::default();

                for (param_type, param_name) in params {
                    let idx = self.nasm.push_supress();
                    var.data.insert(param_name, (param_type.clone(), idx));
                    def_params.push(param_type)
                }

                self.def.fn_table.insert(
                    name,
                    DefinedFunction {
                        return_type: return_type.clone(),
                        params: def_params,
                    },
                );

                for stmt in body {
                    self.codegen_stmt(&return_type, &mut var, stmt)?;
                }

                self.nasm.ret(&return_type)?;

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
            DefinedFunction {
                return_type: return_type.clone(),
                params,
            },
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
                for stmt in statements {
                    self.codegen_stmt(return_type, &mut table, stmt)?;
                }
                Ok(())
            }
        }
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
            ExpressionType::Number(ty) => nasm.push(ty.size_bytes()),
            ExpressionType::Struct(k) => {
                let struc = self.get_struct(k)?;
                let mut first = None;
                for (ty, _) in &struc.fields {
                    first.get_or_insert(self.alloc(nasm, ty)?);
                }
                Ok(match first {
                    Some(first) => first,
                    None => nasm.push(0)?,
                })
            }
            ExpressionType::Ref(_) => nasm.push(8), // references are 64-bit
            ExpressionType::Array(_) => nasm.push(0),
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
                Ok(size)
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

                let idx = self.nasm.push_size_tag("rdx")?;

                Ok((ExpressionType::Array(Box::new(elem_ty)), idx))
            }
            Expression::As(inn) => {
                let (ty, inn) = *inn;
                let (_, idx) = self.codegen_expression(inn)?;
                Ok((ty, idx))
            }
            Expression::FieldAccess(inn) => {
                let (expr, field_name) = *inn;

                let (ExpressionType::Struct(struc_name), expr_idx) =
                    self.codegen_expression(expr)?
                else {
                    return Err(Error::CannotAccessStruct);
                };

                let struc = self.def.get_struct(&struc_name)?;
                let mut fields = struc.fields.iter().enumerate();
                let fields = fields.find(|(_, (_, f))| field_name == *f);

                match fields {
                    Some((offset, (field_ty, _))) => {
                        Ok((field_ty.clone(), expr_idx.offset_by(offset as u16)?))
                    }
                    None => Err(Error::UnknownField(struc_name, field_name)),
                }
            }
            Expression::ArrayAccess(inn) => {
                let (expr, idx_expr) = *inn;

                let (ExpressionType::Ref(expr_ty), arr_idx) = self.codegen_expression(expr)? else {
                    return Err(Error::CannotAccessArray);
                };
                let ExpressionType::Array(elem_ty) = *expr_ty else {
                    return Err(Error::CannotAccessArray);
                };

                let (index_ty, index_idx) = self.codegen_expression(idx_expr)?;
                let index_expected = ExpressionType::Number(NumberType::U16);
                if index_expected != index_ty {
                    return Err(Error::TypeMismatch(index_expected, index_ty));
                }

                self.nasm.idx2addr("rsi", &arr_idx)?;
                self.nasm.idx2addr("rcx", &index_idx)?;
                write_asm!(self.nasm, "mov cx, [rcx+8]")?;

                // push copy of rsi to stack with cx appended
                write_asm!(self.nasm, "sub rsp, 2")?;
                write_asm!(self.nasm, "mov [rsp], cx")?;
                let idx = self.nasm.push_copy_addr()?;
                write_asm!(self.nasm, "add qword [rsp], 2")?;

                Ok((ExpressionType::Ref(elem_ty), idx))
            }
        }
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

    pub fn codegen_binop(
        &mut self,
        (lhs, op, rhs): (Expression, char, Expression),
    ) -> Result<(ExpressionType, Index)> {
        let (lhs_type, lhs) = self.codegen_expression(lhs)?;
        let (rhs_type, rhs) = self.codegen_expression(rhs)?;

        if lhs_type != rhs_type {
            return Err(Error::TypeMismatch(lhs_type, rhs_type));
        }
        let (lhs_type, _) = (lhs_type.into_number()?, rhs_type.into_number()?);

        let ret = match op {
            '+' => self.nasm.add(lhs_type, &lhs, &rhs),
            '-' => self.nasm.sub(lhs_type, &lhs, &rhs),
            _ => return Err(Error::InvalidOperator(op)),
        }?;

        Ok((ExpressionType::Number(lhs_type), ret))
    }

    pub fn codegen_number(&mut self, number: Number) -> Result<(ExpressionType, Index)> {
        let idx = self.nasm.push(number.size_bytes())?;
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
    section_data: Cursor<Vec<u8>>,
}

impl<W: io::Write> Nasm<W> {
    pub fn new(writer: W) -> Self {
        Self {
            writer,
            uniq_index: 0,
            stack_pointer: 0,
            section_data: Cursor::new(Vec::new()),
        }
    }

    pub fn finalize(&mut self) -> Result<()> {
        writeln!(self.writer)?;
        writeln!(self.writer, "section .data")?;
        self.section_data.rewind()?;
        io::copy(&mut self.section_data, &mut self.writer)?;
        Ok(())
    }
}

/*impl<W: io::Write> Nasm<W> {
    pub fn dbg_print(&mut self, msg: &str) -> Result<()> {
        // my debugger output is pretty noise.
        // print a BUNCH so it's easier to spot in the console.
        self.sys_write_msg(2, &format!("\x1b[33mDEBUG: {msg}\x1b[0m\n").repeat(4))
    }

    pub fn sys_write_msg(&mut self, fd: i32, msg: &str) -> Result<()> {
        fn escape_msg(msg: &str) -> String {
            let mut ret = String::from('"');
            for ch in msg.as_bytes() {
                if *ch >= 32 && *ch <= 126 {
                    ret.push(*ch as char);
                } else {
                    ret.push_str("\", ");
                    ret.push_str(&format!("0x{ch:0x}"));
                    ret.push_str(", \"");
                }
            }
            ret.push('"');
            ret.replace(", \"\"", "")
        }

        let uniq_name = format!("sys_write_msg_{}", self.uniq_index);
        self.uniq_index += 1;

        write_dat!(self, "{uniq_name} db {}", escape_msg(msg))?;
        write_dat!(self, "{uniq_name}_len equ $ -{uniq_name}")?;

        self.push_register("eax", RegisterSize::S32)?;
        self.push_register("ebx", RegisterSize::S32)?;
        self.push_register("ecx", RegisterSize::S32)?;
        self.push_register("edx", RegisterSize::S32)?;

        write_asm!(self, "mov eax, 4")?; // sys_write
        write_asm!(self, "mov ebx, {fd}")?; // fd
        write_asm!(self, "mov ecx, {uniq_name}")?; // msg
        write_asm!(self, "mov edx, {uniq_name}_len")?; // length
        write_asm!(self, "int 0x80")?;

        self.pop_register("edx")?;
        self.pop_register("ecx")?;
        self.pop_register("ebx")?;
        self.pop_register("eax")?;

        Ok(())
    }
}*/

impl<W: io::Write> Nasm<W> {
    pub fn add(&mut self, numtype: NumberType, lhs: &Index, rhs: &Index) -> Result<Index> {
        match numtype {
            NumberType::I8 => self.op("add", RegisterSize::S8, lhs, rhs),
            NumberType::I16 => self.op("add", RegisterSize::S16, lhs, rhs),
            NumberType::I32 => self.op("add", RegisterSize::S32, lhs, rhs),
            NumberType::I64 => self.op("add", RegisterSize::S64, lhs, rhs),
            NumberType::U8 => self.op("add", RegisterSize::S8, lhs, rhs),
            NumberType::U16 => self.op("add", RegisterSize::S16, lhs, rhs),
            NumberType::U32 => self.op("add", RegisterSize::S32, lhs, rhs),
            NumberType::U64 => self.op("add", RegisterSize::S64, lhs, rhs),
            NumberType::F32 => self.op("addss", RegisterSize::S32, lhs, rhs),
            NumberType::F64 => self.op("addsd", RegisterSize::S64, lhs, rhs),
        }
    }

    pub fn sub(&mut self, numtype: NumberType, lhs: &Index, rhs: &Index) -> Result<Index> {
        match numtype {
            NumberType::I8 => self.op("sub", RegisterSize::S8, lhs, rhs),
            NumberType::I16 => self.op("sub", RegisterSize::S16, lhs, rhs),
            NumberType::I32 => self.op("sub", RegisterSize::S32, lhs, rhs),
            NumberType::I64 => self.op("sub", RegisterSize::S64, lhs, rhs),
            NumberType::U8 => self.op("sub", RegisterSize::S8, lhs, rhs),
            NumberType::U16 => self.op("sub", RegisterSize::S16, lhs, rhs),
            NumberType::U32 => self.op("sub", RegisterSize::S32, lhs, rhs),
            NumberType::U64 => self.op("sub", RegisterSize::S64, lhs, rhs),
            NumberType::F32 => self.op("subss", RegisterSize::S32, lhs, rhs),
            NumberType::F64 => self.op("subsd", RegisterSize::S64, lhs, rhs),
        }
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

        let idx = self.push(size.size_bytes())?;
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
    pub fn push(&mut self, size: u16) -> Result<Index> {
        let idx = self.stack_pointer;
        self.inc_stack()?;

        let size = size + 8;
        write_asm!(self, "sub rsp, {size} ; push")?;
        write_asm!(self, "mov qword [rsp], {size} ; push")?;

        Ok(Index::new(idx))
    }

    /// Manually push a size tag stored in a register onto the stack.
    /// The `size` parameter is the size of the data including the 8-byte size tag.
    /// `push_size_tag(8)` will store data with a size of `0` because the size tag takes 8-bytes.
    pub fn push_size_tag(&mut self, reg: &str) -> Result<Index> {
        let idx = self.stack_pointer;
        self.inc_stack()?;

        write_asm!(self, "sub rsp, 8 ; push")?;
        write_asm!(self, "mov qword [rsp], {reg} ; push")?;

        Ok(Index::new(idx))
    }

    /// Push data onto the stack but don't increment the stack pointer, return the size allocated.
    /// This is used to push data into an array because the data isn't on the stack but inside the array.
    pub fn push_hidden(&mut self, size: u16) -> Result<u16> {
        let size = size + 8;
        write_asm!(self, "sub rsp, {size} ; push")?;
        write_asm!(self, "mov qword [rsp], {size} ; push")?;
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
        let idx = idx.as_valid()?;
        self.idx2addr_inner(ret_reg, idx.0)?;
        for idx in &idx.1 {
            write_asm!(self, "add {ret_reg}, 8")?;
            self.idx2addr_inner(ret_reg, *idx)?;
        }
        Ok(())
    }

    fn idx2addr_inner(&mut self, ret_reg: &str, idx: u16) -> Result<()> {
        let steps = self.stack_pointer - idx - 1;

        write_asm!(self, "mov {ret_reg}, rsp ; idx2addr (rsp - {steps})")?;
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
        self.ref2addr_jump_back("rsi", "rdx")?;
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
        let idx = idx.as_valid()?;
        let ref_idx = self.push((2 + idx.1.len() * 2) as u16)?;

        let steps = self.stack_pointer - idx.0 - 1;
        write_asm!(self, "mov {s}, bp ; idx2ref")?;
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
        self.uniq_index = 0;
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

#[derive(Clone, Debug)]
pub struct Index(Option<ValidIndex>);

#[derive(Clone, Debug)]
pub struct ValidIndex(u16, Vec<u16>);

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

    pub fn as_valid(&self) -> Result<&ValidIndex> {
        match &self.0 {
            Some(idx) => Ok(idx),
            None => Err(Error::InvalidIndex),
        }
    }

    pub fn last_mut(&mut self) -> Result<&mut u16> {
        let idx = self.valid_mut()?;

        Ok(match idx.1.last_mut() {
            Some(last) => last,
            None => &mut idx.0,
        })
    }

    pub fn offset_by(mut self, amount: u16) -> Result<Self> {
        *self.last_mut()? += amount;
        Ok(self)
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
macro_rules! write_dat {
    ($dst:expr, $($arg:tt)*) => {{
        use std::io::Write;
        write!($dst.section_data, "    ")
            .and_then(|_| writeln!($dst.section_data, $($arg)*))
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
    #[error("type mismatch: expected any reference but got {0}")]
    ExpectedRef(ExpressionType),
    #[error("invalid operator: {0}")]
    InvalidOperator(char),
    #[error("unknown symbol: {0}")]
    UnknownSymbol(String),
    #[error("unknown function: {0}")]
    UnknownFunction(String),
    #[error("arity mismatch: expected {0} parameters but got {0}")]
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
}
