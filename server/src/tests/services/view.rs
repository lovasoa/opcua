use crate::prelude::*;
use crate::services::view::ViewService;
use super::*;

// View service tests

fn make_browse_request(nodes: &[NodeId], max_references_per_node: usize, browse_direction: BrowseDirection, reference_type: ReferenceTypeId) -> BrowseRequest {
    let request_header = make_request_header();
    let nodes_to_browse = nodes.iter().map(|n| {
        BrowseDescription {
            node_id: n.clone(),
            browse_direction,
            reference_type_id: reference_type.into(),
            include_subtypes: true,
            node_class_mask: 0xff,
            result_mask: 0xff,
        }
    }).collect();
    BrowseRequest {
        request_header,
        view: ViewDescription {
            view_id: NodeId::null(),
            timestamp: DateTime::now(),
            view_version: 0,
        },
        requested_max_references_per_node: max_references_per_node as u32,
        nodes_to_browse: Some(nodes_to_browse),
    }
}

fn make_browse_next_request(continuation_point: &ByteString, release_continuation_points: bool) -> BrowseNextRequest {
    let request_header = make_request_header();
    BrowseNextRequest {
        request_header,
        release_continuation_points,
        continuation_points: if continuation_point.is_null() { None } else { Some(vec![continuation_point.clone()]) },
    }
}

fn verify_references_to_many_vars(references: &[ReferenceDescription], expected_size: usize, start_idx: usize) {
    // Verify that the reference descriptions point at sequential vars
    assert_eq!(references.len(), expected_size);
    for (i, r) in references.iter().enumerate() {
        let expected_node_id = NodeId::new(1, format!("v{}", i + start_idx));
        assert_eq!(r.node_id.node_id, expected_node_id);
    }
}

fn do_browse(vs: &ViewService, session: &mut Session, address_space: &AddressSpace, nodes: &[NodeId], max_references_per_node: usize) -> BrowseResponse {
    let request = make_browse_request(nodes, max_references_per_node, BrowseDirection::Forward, ReferenceTypeId::Organizes);
    let result = vs.browse(session, address_space, &request);
    assert!(result.is_ok());
    supported_message_as!(result.unwrap(), BrowseResponse)
}

fn do_browse_next(vs: &ViewService, session: &mut Session, address_space: &AddressSpace, continuation_point: &ByteString, release_continuation_points: bool) -> BrowseNextResponse {
    let request = make_browse_next_request(continuation_point, release_continuation_points);
    let result = vs.browse_next(session, address_space, &request);
    assert!(result.is_ok());
    supported_message_as!(result.unwrap(), BrowseNextResponse)
}

#[test]
fn browse() {
    let st = ServiceTest::new();
    let (_, mut session) = st.get_server_state_and_session();

    let vs = ViewService::new();

    let mut address_space = st.address_space.write().unwrap();
    add_sample_vars_to_address_space(&mut address_space);

    let nodes: Vec<NodeId> = vec![ObjectId::RootFolder.into()];
    let response = do_browse(&vs, &mut session, &address_space, &nodes, 1000);
    assert!(response.results.is_some());

    let results = response.results.unwrap();
    assert_eq!(results.len(), 1);

    assert!(results[0].references.is_some());
    let references = results[0].references.as_ref().unwrap();
    assert_eq!(references.len(), 3);

    // Expect to see refs to
    // Objects/
    // Types/
    // Views/

    let r1 = &references[0];
    assert_eq!(r1.browse_name, QualifiedName::new(0, "Objects"));
    let r2 = &references[1];
    assert_eq!(r2.browse_name, QualifiedName::new(0, "Types"));
    let r3 = &references[2];
    assert_eq!(r3.browse_name, QualifiedName::new(0, "Views"));
}

#[test]
fn browse_next() {
    // Set up a server with more nodes than can fit in a response to test Browse, BrowseNext response
    let st = ServiceTest::new();
    let (_, mut session) = st.get_server_state_and_session();

    let mut address_space = st.address_space.write().unwrap();
    let parent_node_id = add_many_vars_to_address_space(&mut address_space, 100).0;
    let nodes = vec![parent_node_id.clone()];

    let vs = ViewService::new();

    // Browse with requested_max_references_per_node = 101, expect 100 results, no continuation point
    {
        let response = do_browse(&vs, &mut session, &address_space, &nodes, 101);
        assert!(response.results.is_some());
        let r1 = &response.results.unwrap()[0];
        let references = r1.references.as_ref().unwrap();
        assert!(r1.continuation_point.is_null());
        verify_references_to_many_vars(references, 100, 0);
    }

    // Browse with requested_max_references_per_node = 100, expect 100 results, no continuation point
    {
        let response = do_browse(&vs, &mut session, &address_space, &nodes, 100);
        let r1 = &response.results.unwrap()[0];
        let references = r1.references.as_ref().unwrap();
        assert!(r1.continuation_point.is_null());
        verify_references_to_many_vars(references, 100, 0);
    }

    // Browse with requested_max_references_per_node = 99 expect 99 results and a continuation point
    // Browse next with continuation point, expect 1 result leaving off from last continuation point
    let continuation_point = {
        // Get first 99
        let response = do_browse(&vs, &mut session, &address_space, &nodes, 99);
        let r1 = &response.results.unwrap()[0];
        let references = r1.references.as_ref().unwrap();
        assert!(!r1.continuation_point.is_null());
        verify_references_to_many_vars(references, 99, 0);

        // Expect continuation point and browse next to return last var and no more continuation point
        let response = do_browse_next(&vs, &mut session, &address_space, &r1.continuation_point, false);
        let r2 = &response.results.unwrap()[0];
        assert!(r2.continuation_point.is_null());
        let references = r2.references.as_ref().unwrap();
        verify_references_to_many_vars(references, 1, 99);

        // Browse next again with same continuation point, expect same 1 result
        let response = do_browse_next(&vs, &mut session, &address_space, &r1.continuation_point, false);
        let r2 = &response.results.unwrap()[0];
        assert!(r2.continuation_point.is_null());
        let references = r2.references.as_ref().unwrap();
        verify_references_to_many_vars(references, 1, 99);

        r1.continuation_point.clone()
    };

    // Browse next and release the previous continuation points, expect Null result
    {
        let response = do_browse_next(&vs, &mut session, &address_space, &continuation_point, true);
        assert!(response.results.is_none());

        // Browse next again with same continuation point, expect BadContinuationPointInvalid
        let response = do_browse_next(&vs, &mut session, &address_space, &continuation_point, false);
        let r1 = &response.results.unwrap()[0];
        assert_eq!(r1.status_code, StatusCode::BadContinuationPointInvalid);
    }

    // Browse with 35 expect continuation point cp1
    // Browse next with cp1 with 35 expect cp2
    // Browse next with cp2 expect 30 results
    {
        // Get first 35
        let response = do_browse(&vs, &mut session, &address_space, &nodes, 35);
        let r1 = &response.results.unwrap()[0];
        let references = r1.references.as_ref().unwrap();
        assert!(!r1.continuation_point.is_null());
        verify_references_to_many_vars(references, 35, 0);

        // Expect continuation point and browse next to return last var and no more continuation point
        let response = do_browse_next(&vs, &mut session, &address_space, &r1.continuation_point, false);
        let r2 = &response.results.unwrap()[0];
        assert!(!r2.continuation_point.is_null());
        let references = r2.references.as_ref().unwrap();
        verify_references_to_many_vars(references, 35, 35);

        // Expect continuation point and browse next to return last var and no more continuation point
        let response = do_browse_next(&vs, &mut session, &address_space, &r2.continuation_point, false);
        let r3 = &response.results.unwrap()[0];
        assert!(r3.continuation_point.is_null());
        let references = r3.references.as_ref().unwrap();
        verify_references_to_many_vars(references, 30, 70);
    }

    // Modify address space so existing continuation point is invalid
    // Browse next with continuation point, expect BadContinuationPointInvalid
    {
        use std::thread;
        use std::time::Duration;

        // Sleep a bit, modify the address space so the old continuation point is out of date
        thread::sleep(Duration::from_millis(50));
        {
            let var_name = "xxxx";
            let node_id = NodeId::new(1, var_name);
            let var = Variable::new(&node_id, var_name, var_name, "", 200 as i32);
            let _ = address_space.add_variable(var, &parent_node_id);
        }

        // Browsing with the old continuation point should fail
        let response = do_browse_next(&vs, &mut session, &address_space, &continuation_point, false);
        let r1 = &response.results.unwrap()[0];
        assert_eq!(r1.status_code, StatusCode::BadContinuationPointInvalid);
    }
}

#[test]
fn translate_browse_paths_to_node_ids() {
    let st = ServiceTest::new();

    // This is a very basic test of this service. It wants to find the relative path from root to the
    // Objects folder and ensure that it comes back in the result

    let browse_paths = vec![
        BrowsePath {
            starting_node: ObjectId::RootFolder.into(),
            relative_path: RelativePath {
                elements: Some(vec![
                    RelativePathElement {
                        reference_type_id: ReferenceTypeId::HasChild.into(),
                        is_inverse: false,
                        include_subtypes: true,
                        target_name: QualifiedName::new(0, "Objects"),
                    }
                ]),
            },
        }
    ];

    let request = TranslateBrowsePathsToNodeIdsRequest {
        request_header: make_request_header(),
        browse_paths: Some(browse_paths),
    };

    let vs = ViewService::new();
    let address_space = st.address_space.read().unwrap();
    let result = vs.translate_browse_paths_to_node_ids(&address_space, &request);
    assert!(result.is_ok());
    let result: TranslateBrowsePathsToNodeIdsResponse = supported_message_as!(result.unwrap(), TranslateBrowsePathsToNodeIdsResponse);

    debug!("result = {:#?}", result);

    let results = result.results.unwrap();
    assert_eq!(results.len(), 1);
    let r1 = &results[0];

    // TODO broken
    /*    let targets = r1.targets.as_ref().unwrap();
        assert_eq!(targets.len(), 1);
        let t1 = &targets[0];
        assert_eq!(&t1.target_id.node_id, &AddressSpace::objects_folder_id()); */
}

///
/// * `/` - The forward slash character indicates that the Server is to follow any subtype of HierarchicalReferences.
/// * `.` - The period (dot) character indicates that the Server is to follow any subtype of a Aggregates ReferenceType.
/// * `<[#!ns:]ReferenceType>` - A string delimited by the ‘<’ and ‘>’ symbols specifies the BrowseName of a ReferenceType to follow.
///   By default, any References of the subtypes the ReferenceType are followed as well. A ‘#’ placed in front of the BrowseName indicates
///   that subtypes should not be followed.
///   A ‘!’ in front of the BrowseName is used to indicate that the inverse Reference should be followed.
///   The BrowseName may be qualified with a namespace index (indicated by a numeric prefix followed by a colon).
///   This namespace index is used specify the namespace component of the BrowseName for the ReferenceType. If the namespace prefix is omitted then namespace index 0 is used.
/// * `[ns:]BrowseName` - A string that follows a ‘/’, ‘.’ or ‘>’ symbol specifies the BrowseName of a target
///   Node to return or follow. This BrowseName may be prefixed by its namespace index. If the namespace prefix
///   is omitted then namespace index 0 is used.
///   Omitting the final BrowseName from a path is equivalent to a wildcard operation that matches all
///   Nodes which are the target of the Reference specified by the path.
/// * `&` - The & sign character is the escape character. It is used to specify reserved characters
///   that appear within a BrowseName. A reserved character is escaped by inserting the ‘&’ in front of it.
const xxxx: u32 = 0;

/*

https://github.com/node-opcua/node-opcua/blob/68b1b57dec23a45148468fbea89ab71a39f9042f/test/end_to_end/u_test_e2e_translateBrowsePath.js

// find nodeId of Root.Objects.server.status.buildInfo
                var browsePath = [
                    makeBrowsePath("RootFolder","/Objects/Server"),
                    makeBrowsePath("RootFolder","/Objects/Server.ServerStatus"),
                    makeBrowsePath("RootFolder","/Objects/Server.ServerStatus.BuildInfo"),
                    makeBrowsePath("RootFolder","/Objects/Server.ServerStatus.BuildInfo.ProductName"),
                    makeBrowsePath("RootFolder","/Objects/Server.ServerStatus.BuildInfo."), // missing TargetName !
                    makeBrowsePath("RootFolder","/Objects.Server"), // intentional error usign . instead of /
                    makeBrowsePath("RootFolder","/Objects/2:MatrikonOPC Simulation Server (DA)") // va
                ];

                //xx console.log("browsePath ", browsePath[0].toString({addressSpace: server.engine.addressSpace}));

                session.translateBrowsePath(browsePath, function (err, results) {

                    if (!err) {
                        results.length.should.eql(browsePath.length);
                        //xx console.log(results[0].toString());

                        results[0].statusCode.should.eql(StatusCodes.Good);
                        results[0].targets.length.should.eql(1);
                        results[0].targets[0].targetId.toString().should.eql("ns=0;i=2253");
                        results[0].targets[0].targetId.value.should.eql(opcua.ObjectIds.Server);

                        //xx console.log(results[1].toString());
                        results[1].statusCode.should.eql(StatusCodes.Good);
                        results[1].targets.length.should.eql(1);
                        results[1].targets[0].targetId.toString().should.eql("ns=0;i=2256");
                        results[1].targets[0].targetId.value.should.eql(opcua.VariableIds.Server_ServerStatus);

                        //xx console.log(results[2].toString());
                        results[2].statusCode.should.eql(StatusCodes.Good);
                        results[2].targets.length.should.eql(1);
                        results[2].targets[0].targetId.toString().should.eql("ns=0;i=2260");
                        results[2].targets[0].targetId.value.should.eql(opcua.VariableIds.Server_ServerStatus_BuildInfo);

                        //xx console.log(results[3].toString());
                        results[3].statusCode.should.eql(StatusCodes.Good);
                        results[3].targets.length.should.eql(1);
                        results[3].targets[0].targetId.toString().should.eql("ns=0;i=2261");
                        results[3].targets[0].targetId.value.should.eql(opcua.VariableIds.Server_ServerStatus_BuildInfo_ProductName);

                        // missing browseName on last element of the relativepath => ERROR
                        results[4].statusCode.should.eql(StatusCodes.BadBrowseNameInvalid);

                        results[5].statusCode.should.eql(StatusCodes.BadNoMatch);

                        results[6].statusCode.should.eql(StatusCodes.BadNoMatch);

}
*/

