

// everything below is auto generated

procedure {:inline 1} ReadValue0(p: Path, i: int, v: Value) returns (v': Value)
{
    var e: Edge;
    if (i == size#Path(p)) {
        v' := v;
    } else {
        assert false;
    }
}

procedure {:inline 1} ReadValue1(p: Path, i: int, v: Value) returns (v': Value)
{
    var e: Edge;
    if (i == size#Path(p)) {
        v' := v;
    } else {
        e := p#Path(p)[i];
        v' := m#Map(v)[e];
        if (is#Vector(v)) { v' := v#Vector(v)[e]; }
        call v' := ReadValue0(p, i+1, v');
    }
}

procedure {:inline 1} ReadValue2(p: Path, i: int, v: Value) returns (v': Value)
{
    var e: Edge;
    if (i == size#Path(p)) {
        v' := v;
    } else {
        e := p#Path(p)[i];
        v' := m#Map(v)[e];
        if (is#Vector(v)) { v' := v#Vector(v)[e]; }
        call v' := ReadValue1(p, i+1, v');
    }
}

procedure {:inline 1} ReadValueMax(p: Path, i: int, v: Value) returns (v': Value)
{
    var e: Edge;
    if (i == size#Path(p)) {
        v' := v;
    } else {
        e := p#Path(p)[i];
        v' := m#Map(v)[e];
        if (is#Vector(v)) { v' := v#Vector(v)[e]; }
        call v' := ReadValue2(p, i+1, v');
    }
}

procedure {:inline 1} UpdateValue0(p: Path, i: int, v: Value, new_v: Value) returns (v': Value)
{
    var e: Edge;
    if (i == size#Path(p)) {
        v' := new_v;
    } else {
        assert false;
    }
}

procedure {:inline 1} UpdateValue1(p: Path, i: int, v: Value, new_v: Value) returns (v': Value)
{
    var e: Edge;
    if (i == size#Path(p)) {
        v' := new_v;
    } else {
        e := p#Path(p)[i];
        v' := m#Map(v)[e];
        if (is#Vector(v)) { v' := v#Vector(v)[e]; }
        call v' := UpdateValue0(p, i+1, v', new_v);
        if (is#Map(v)) { v' := Map(m#Map(v)[e := v']);}
        if (is#Vector(v)) { v' := Vector(v#Vector(v)[e := v'], l#Vector(v));}
    }
}

procedure {:inline 1} UpdateValue2(p: Path, i: int, v: Value, new_v: Value) returns (v': Value)
{
    var e: Edge;
    if (i == size#Path(p)) {
        v' := new_v;
    } else {
        e := p#Path(p)[i];
        v' := m#Map(v)[e];
        if (is#Vector(v)) { v' := v#Vector(v)[e]; }
        call v' := UpdateValue1(p, i+1, v', new_v);
        if (is#Map(v)) { v' := Map(m#Map(v)[e := v']);}
        if (is#Vector(v)) { v' := Vector(v#Vector(v)[e := v'], l#Vector(v));}
    }
}

procedure {:inline 1} UpdateValueMax(p: Path, i: int, v: Value, new_v: Value) returns (v': Value)
{
    var e: Edge;
    if (i == size#Path(p)) {
        v' := new_v;
    } else {
        e := p#Path(p)[i];
        v' := m#Map(v)[e];
        if (is#Vector(v)) { v' := v#Vector(v)[e]; }
        call v' := UpdateValue2(p, i+1, v', new_v);
        if (is#Map(v)) { v' := Map(m#Map(v)[e := v']);}
        if (is#Vector(v)) { v' := Vector(v#Vector(v)[e := v'], l#Vector(v));}
    }
}

procedure {:inline 1} TestControlFlow_branch_once (arg0: Value) returns (ret0: Value)
{
    // declare local variables
    var t0: Value; // bool
    var t1: Value; // bool
    var t2: Value; // int
    var t3: Value; // int
    var t4: Value; // int
    var t5: Value; // int

    var tmp: Value;
    var old_size: int;
    assume !abort_flag;

    // assume arguments are of correct types
    assume is#Boolean(arg0);

    old_size := m_size;
    m_size := m_size + 6;
    m := Memory(domain#Memory(m)[0+old_size := true], contents#Memory(m)[0+old_size :=  arg0]);

    // bytecode translation starts here
    call tmp := CopyOrMoveValue(contents#Memory(m)[old_size+0]);
    m := Memory(domain#Memory(m)[1+old_size := true], contents#Memory(m)[1+old_size := tmp]);

    tmp := contents#Memory(m)[old_size + 1];
if (!b#Boolean(tmp)) { goto Label_6; }

    call tmp := LdConst(1);
    m := Memory(domain#Memory(m)[2+old_size := true], contents#Memory(m)[2+old_size := tmp]);

    call tmp := LdConst(2);
    m := Memory(domain#Memory(m)[3+old_size := true], contents#Memory(m)[3+old_size := tmp]);

    call tmp := Add(contents#Memory(m)[old_size+2], contents#Memory(m)[old_size+3]);
    m := Memory(domain#Memory(m)[4+old_size := true], contents#Memory(m)[4+old_size := tmp]);

    ret0 := contents#Memory(m)[old_size+4];
    return;

Label_6:
    call tmp := LdConst(0);
    m := Memory(domain#Memory(m)[5+old_size := true], contents#Memory(m)[5+old_size := tmp]);

    ret0 := contents#Memory(m)[old_size+5];
    return;

}

procedure TestControlFlow_branch_once_verify (arg0: Value) returns (ret0: Value)
{
    call ret0 := TestControlFlow_branch_once(arg0);
}
